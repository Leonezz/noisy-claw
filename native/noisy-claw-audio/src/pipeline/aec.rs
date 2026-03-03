use tokio::sync::mpsc;

use crate::aec::EchoCanceller;

use super::AudioFrame;
use super::dump;

pub enum Control {
    ResetBuffers,
    Shutdown,
}

pub struct Handle {
    pub control_tx: mpsc::Sender<Control>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Handle {
    pub async fn reset_buffers(&self) {
        let _ = self.control_tx.send(Control::ResetBuffers).await;
    }

    pub async fn shutdown(mut self) {
        let _ = self.control_tx.send(Control::Shutdown).await;
        if let Some(join) = self.join.take() {
            // Wait for the AEC thread to finish without blocking the async runtime
            let _ = tokio::task::spawn_blocking(move || join.join()).await;
        }
    }
}

/// Spawn the AEC node in a dedicated OS thread.
///
/// EchoCanceller runs in a dedicated OS thread to avoid blocking the
/// main tokio runtime with CPU-intensive audio processing (AEC + NS + HPF).
///
/// Inputs:
///   - `capture_rx`: raw mic audio from CaptureNode
///   - `render_rx`:  speaker output reference from OutputNode
///
/// Output:
///   - `cleaned_tx`: echo-cancelled audio → VadNode
pub fn spawn(
    capture_rx: mpsc::UnboundedReceiver<AudioFrame>,
    render_rx: mpsc::UnboundedReceiver<AudioFrame>,
    cleaned_tx: mpsc::UnboundedSender<AudioFrame>,
) -> Handle {
    let (ctl_tx, ctl_rx) = mpsc::channel(16);

    let join = std::thread::Builder::new()
        .name("aec-node".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to create AEC runtime");

            rt.block_on(aec_loop(capture_rx, render_rx, cleaned_tx, ctl_rx));
        })
        .expect("failed to spawn AEC thread");

    Handle {
        control_tx: ctl_tx,
        join: Some(join),
    }
}

/// Compute RMS energy of a signal in dB.
fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -100.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    let rms = (sum_sq / samples.len() as f32).sqrt();
    if rms < 1e-10 {
        -100.0
    } else {
        20.0 * rms.log10()
    }
}

async fn aec_loop(
    mut capture_rx: mpsc::UnboundedReceiver<AudioFrame>,
    mut render_rx: mpsc::UnboundedReceiver<AudioFrame>,
    cleaned_tx: mpsc::UnboundedSender<AudioFrame>,
    mut ctl_rx: mpsc::Receiver<Control>,
) {
    let mut ec = match EchoCanceller::new() {
        Ok(ec) => {
            tracing::info!("AEC node: initialized");
            Some(ec)
        }
        Err(e) => {
            tracing::warn!(%e, "AEC node: init failed, running in passthrough mode");
            None
        }
    };

    let mut log_counter: u32 = 0;

    // Convergence blanking: when render transitions from silence to audio,
    // the AEC needs time to adapt its echo path estimate. During this period,
    // residual echo can leak through and trigger false VAD detections.
    // We zero the cleaned output for CONVERGENCE_FRAMES to prevent this.
    const CONVERGENCE_FRAMES: u32 = 25; // ~400ms at ~16ms/frame
    let mut render_was_active = false;
    let mut convergence_countdown: u32 = 0;

    loop {
        tokio::select! {
            Some(ctl) = ctl_rx.recv() => {
                match ctl {
                    Control::ResetBuffers => {
                        if let Some(ref mut ec) = ec {
                            ec.reset_buffers();
                            tracing::info!("AEC node: buffers reset");
                        }
                    }
                    Control::Shutdown => {
                        tracing::info!("AEC node: shutdown");
                        break;
                    }
                }
            }

            // Process capture frames — eagerly drain render reference first.
            // Render frames are ONLY consumed here (not in a separate branch)
            // to guarantee they are fed to the AEC immediately before the
            // corresponding capture frame, preserving correct timing alignment.
            Some(frame) = capture_rx.recv() => {
                dump::write("capture", &frame.samples, frame.sample_rate);

                let mut render_energy = -100.0_f32;
                let mut render_frames: u32 = 0;
                if let Some(ref mut ec) = ec {
                    while let Ok(ref_frame) = render_rx.try_recv() {
                        dump::write("render", &ref_frame.samples, ref_frame.sample_rate);
                        render_energy = render_energy.max(rms_db(&ref_frame.samples));
                        ec.feed_render(&ref_frame.samples, ref_frame.sample_rate);
                        render_frames += 1;
                    }
                }

                let capture_energy = rms_db(&frame.samples);

                let mut cleaned_samples = if let Some(ref mut ec) = ec {
                    let result = ec.process_capture(&frame.samples, frame.sample_rate);
                    if result.is_empty() { frame.samples.clone() } else { result }
                } else {
                    frame.samples.clone()
                };

                // Detect render start → apply convergence blanking
                let render_is_active = render_energy > -90.0;
                if render_is_active && !render_was_active {
                    convergence_countdown = CONVERGENCE_FRAMES;
                    tracing::info!(
                        convergence_frames = CONVERGENCE_FRAMES,
                        "AEC node: render started, convergence blanking active"
                    );
                }
                render_was_active = render_is_active;

                if convergence_countdown > 0 {
                    convergence_countdown -= 1;
                    cleaned_samples.iter_mut().for_each(|s| *s = 0.0);
                }

                let cleaned_energy = rms_db(&cleaned_samples);

                // Log energy levels periodically (~every 500ms at typical frame rates)
                log_counter += 1;
                if log_counter % 15 == 0 {
                    tracing::info!(
                        capture_db = format!("{:.1}", capture_energy),
                        render_db = format!("{:.1}", render_energy),
                        cleaned_db = format!("{:.1}", cleaned_energy),
                        suppression_db = format!("{:.1}", capture_energy - cleaned_energy),
                        render_frames,
                        convergence_countdown,
                        "AEC node: energy"
                    );
                }

                dump::write("aec_out", &cleaned_samples, frame.sample_rate);

                let _ = cleaned_tx.send(AudioFrame {
                    samples: cleaned_samples,
                    sample_rate: frame.sample_rate,
                    vad: None,
                });
            }
        }
    }
}
