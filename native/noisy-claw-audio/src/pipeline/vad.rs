use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::protocol::Event;
use crate::vad::VoiceActivityDetector;

use super::{AudioFrame, VadEvent};

/// ~192ms at 32ms/window — number of consecutive VAD-positive frames
/// required before confirming barge-in during TTS playback.
const BARGE_IN_FRAME_COUNT: u32 = 6;

pub enum Control {
    SetThreshold(f32),
    Reset,
    Shutdown,
}

pub struct Handle {
    pub control_tx: mpsc::Sender<Control>,
    initialized: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

impl Handle {
    pub async fn set_threshold(&self, threshold: f32) {
        let _ = self.control_tx.send(Control::SetThreshold(threshold)).await;
    }

    pub async fn reset(&self) {
        let _ = self.control_tx.send(Control::Reset).await;
    }

    pub async fn shutdown(self) {
        let _ = self.control_tx.send(Control::Shutdown).await;
        let _ = self.join.await;
    }

    /// Whether the VAD model was loaded successfully.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }
}

/// Spawn the VAD node.
///
/// Inputs:
///   - `audio_rx`: cleaned audio from AecNode
///
/// Outputs:
///   - `audio_passthrough_tx`: forwards audio to SttNode
///   - `vad_event_tx`:         VAD transitions to SttNode
///   - `event_tx`:             IPC events (Event::Vad) to stdout
///   - `barge_in_tx`:          fires when barge-in is confirmed during TTS
///
/// Observes:
///   - `is_speaking_tts`: pipeline-wide TTS state for hybrid gate
pub fn spawn(
    mut audio_rx: mpsc::UnboundedReceiver<AudioFrame>,
    audio_passthrough_tx: mpsc::UnboundedSender<AudioFrame>,
    vad_event_tx: mpsc::Sender<VadEvent>,
    event_tx: mpsc::Sender<Event>,
    barge_in_tx: mpsc::Sender<()>,
    is_speaking_tts: watch::Receiver<bool>,
    model_path: PathBuf,
    initial_threshold: f32,
) -> Handle {
    let (ctl_tx, mut ctl_rx) = mpsc::channel(16);
    let initialized = Arc::new(AtomicBool::new(false));

    // Load model synchronously so is_initialized() is accurate
    // before any commands arrive.
    let vad = match VoiceActivityDetector::new(&model_path, initial_threshold) {
        Ok(v) => {
            tracing::info!(path = %model_path.display(), "VAD node: model loaded");
            initialized.store(true, Ordering::SeqCst);
            Some(v)
        }
        Err(e) => {
            tracing::warn!(
                %e, path = %model_path.display(),
                "VAD node: init failed, running in passthrough mode"
            );
            None
        }
    };

    let join = tokio::spawn(async move {
        let mut vad = vad;
        tracing::info!("VAD node: task started");

        let mut consecutive_speech_frames: u32 = 0;
        let mut was_speaking = false;
        let mut log_counter: u32 = 0;

        loop {
            tokio::select! {
                Some(ctl) = ctl_rx.recv() => {
                    match ctl {
                        Control::SetThreshold(t) => {
                            if let Some(ref mut v) = vad {
                                v.set_threshold(t);
                                tracing::info!(threshold = t, "VAD node: threshold updated");
                            }
                        }
                        Control::Reset => {
                            if let Some(ref mut v) = vad {
                                v.reset();
                            }
                            consecutive_speech_frames = 0;
                            was_speaking = false;
                            tracing::info!("VAD node: reset");
                        }
                        Control::Shutdown => {
                            tracing::info!("VAD node: shutdown");
                            break;
                        }
                    }
                }

                Some(frame) = audio_rx.recv() => {
                    let speaking_tts = *is_speaking_tts.borrow();

                    // Always forward audio to STT
                    let _ = audio_passthrough_tx.send(frame.clone());

                    // Run VAD inference
                    let Some(ref mut v) = vad else { continue };
                    let results = match v.process(&frame.samples) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!(%e, "VAD node: processing failed");
                            continue;
                        }
                    };

                    for w in results {
                        if speaking_tts {
                            log_counter += 1;

                            // Log every window's probability during TTS (~every 32ms)
                            // so we can see if AEC is suppressing or threshold is too high.
                            // Use info level so it shows up with default RUST_LOG.
                            if log_counter % 15 == 0 {
                                tracing::info!(
                                    prob = format!("{:.3}", w.speech_prob),
                                    is_speech = w.is_speech,
                                    consecutive = consecutive_speech_frames,
                                    threshold = v.threshold(),
                                    "VAD node: TTS gate"
                                );
                            }

                            // Hybrid gate: require sustained speech for barge-in
                            if w.is_speech {
                                consecutive_speech_frames += 1;
                                if consecutive_speech_frames == 1 {
                                    tracing::info!(
                                        prob = format!("{:.3}", w.speech_prob),
                                        "VAD node: speech frame detected during TTS"
                                    );
                                }
                                if consecutive_speech_frames >= BARGE_IN_FRAME_COUNT
                                    && !was_speaking
                                {
                                    tracing::info!(
                                        consecutive_speech_frames,
                                        prob = format!("{:.3}", w.speech_prob),
                                        "VAD node: barge-in triggered"
                                    );
                                    let _ = barge_in_tx.send(()).await;
                                    let _ = event_tx.send(Event::Vad { speaking: true }).await;
                                    let _ = vad_event_tx
                                        .send(VadEvent { speaking: true })
                                        .await;
                                    was_speaking = true;
                                }
                            } else {
                                if consecutive_speech_frames > 0 {
                                    tracing::info!(
                                        consecutive_speech_frames,
                                        prob = format!("{:.3}", w.speech_prob),
                                        "VAD node: speech streak broken during TTS"
                                    );
                                }
                                consecutive_speech_frames = 0;
                                if was_speaking {
                                    tracing::info!("VAD node: speech ended during TTS");
                                    let _ =
                                        event_tx.send(Event::Vad { speaking: false }).await;
                                    let _ = vad_event_tx
                                        .send(VadEvent { speaking: false })
                                        .await;
                                    was_speaking = false;
                                }
                            }
                        } else {
                            log_counter = 0;
                            // Normal mode: emit on transitions only
                            if let Some(speaking) = w.transition {
                                tracing::info!(
                                    speaking,
                                    prob = format!("{:.3}", w.speech_prob),
                                    "VAD node: transition"
                                );
                                let _ = event_tx.send(Event::Vad { speaking }).await;
                                let _ = vad_event_tx.send(VadEvent { speaking }).await;
                                was_speaking = speaking;
                            }
                        }
                    }
                }
            }
        }
    });

    Handle {
        control_tx: ctl_tx,
        initialized,
        join,
    }
}
