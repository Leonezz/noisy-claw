use tokio::sync::mpsc;

use crate::aec::EchoCanceller;

use super::AudioFrame;

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
/// EchoCanceller (aec3) is not Send, so we run it in its own thread
/// with a single-threaded tokio runtime for channel communication.
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

    loop {
        tokio::select! {
            Some(ctl) = ctl_rx.recv() => {
                match ctl {
                    Control::ResetBuffers => {
                        if let Some(ref mut ec) = ec {
                            ec.reset_buffers();
                            tracing::debug!("AEC node: buffers reset");
                        }
                    }
                    Control::Shutdown => {
                        tracing::info!("AEC node: shutdown");
                        break;
                    }
                }
            }

            // Process capture frames — eagerly drain render reference first
            Some(frame) = capture_rx.recv() => {
                if let Some(ref mut ec) = ec {
                    while let Ok(ref_frame) = render_rx.try_recv() {
                        ec.feed_render(&ref_frame.samples, ref_frame.sample_rate);
                    }
                }

                let cleaned_samples = if let Some(ref mut ec) = ec {
                    let result = ec.process_capture(&frame.samples, frame.sample_rate);
                    if result.is_empty() { frame.samples.clone() } else { result }
                } else {
                    frame.samples.clone()
                };

                let _ = cleaned_tx.send(AudioFrame {
                    samples: cleaned_samples,
                    sample_rate: frame.sample_rate,
                });
            }

            // Feed render reference when no capture is arriving
            Some(ref_frame) = render_rx.recv() => {
                if let Some(ref mut ec) = ec {
                    ec.feed_render(&ref_frame.samples, ref_frame.sample_rate);
                }
            }
        }
    }
}
