use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::output::StreamingOutput;

use super::{AudioFrame, OutputMessage, OutputNodeEvent};

pub enum Control {
    Shutdown,
}

pub struct Handle {
    pub control_tx: mpsc::Sender<Control>,
    join: JoinHandle<()>,
}

impl Handle {
    pub async fn shutdown(self) {
        let _ = self.control_tx.send(Control::Shutdown).await;
        let _ = self.join.await;
    }
}

/// Spawn the output node.
///
/// Input:
///   - `msg_rx`:         OutputMessages from TtsNode / orchestrator
///
/// Outputs:
///   - `render_ref_tx`:  speaker reference audio → AecNode
///   - `internal_tx`:    OutputNodeEvent::SpeakDone → orchestrator
pub fn spawn(
    mut msg_rx: mpsc::Receiver<OutputMessage>,
    render_ref_tx: mpsc::UnboundedSender<AudioFrame>,
    internal_tx: mpsc::Sender<OutputNodeEvent>,
) -> Handle {
    let (ctl_tx, mut ctl_rx) = mpsc::channel(16);

    let join = tokio::spawn(async move {
        let mut streaming_output: Option<StreamingOutput> = None;
        let mut tts_sample_rate: u32 = 16000;
        // Handle for the render-reference forwarding task
        let mut ref_fwd_handle: Option<JoinHandle<()>> = None;

        loop {
            tokio::select! {
                Some(ctl) = ctl_rx.recv() => {
                    match ctl {
                        Control::Shutdown => {
                            if let Some(ref mut out) = streaming_output {
                                out.stop();
                            }
                            streaming_output = None;
                            if let Some(h) = ref_fwd_handle.take() {
                                h.abort();
                            }
                            tracing::info!("output node: shutdown");
                            break;
                        }
                    }
                }

                Some(msg) = msg_rx.recv() => {
                    match msg {
                        OutputMessage::StartSession { sample_rate } => {
                            // Clean up previous session if any
                            if let Some(ref mut out) = streaming_output {
                                out.stop();
                            }
                            if let Some(h) = ref_fwd_handle.take() {
                                h.abort();
                            }

                            tts_sample_rate = sample_rate;
                            match StreamingOutput::new(sample_rate) {
                                Ok((out, mut ref_rx)) => {
                                    let output_rate = out.sample_rate();
                                    streaming_output = Some(out);

                                    // Spawn render-reference forwarder
                                    let ref_tx = render_ref_tx.clone();
                                    ref_fwd_handle = Some(tokio::spawn(async move {
                                        while let Some(samples) = ref_rx.recv().await {
                                            let _ = ref_tx.send(AudioFrame {
                                                samples,
                                                sample_rate: output_rate,
                                            });
                                        }
                                    }));

                                    tracing::info!(
                                        sample_rate,
                                        output_rate,
                                        "output node: session started"
                                    );
                                }
                                Err(e) => {
                                    tracing::error!(
                                        %e, "output node: streaming output init failed"
                                    );
                                    streaming_output = None;
                                }
                            }
                        }

                        OutputMessage::AudioChunk(samples) => {
                            if let Some(ref mut out) = streaming_output {
                                let written = out.write_samples(&samples, tts_sample_rate);
                                tracing::debug!(
                                    chunk_samples = samples.len(),
                                    written,
                                    "output node: audio chunk written"
                                );
                            }
                        }

                        OutputMessage::FinishSession => {
                            if let Some(ref out) = streaming_output {
                                out.finish();

                                // Poll until buffer drains
                                let playing = out.playing_flag();
                                loop {
                                    if !playing.load(Ordering::SeqCst) {
                                        break;
                                    }
                                    tokio::time::sleep(Duration::from_millis(50)).await;
                                }

                                tracing::info!("output node: buffer drained");
                            }

                            // Clean up
                            streaming_output = None;
                            if let Some(h) = ref_fwd_handle.take() {
                                h.abort();
                            }

                            let _ = internal_tx.send(OutputNodeEvent::SpeakDone).await;
                        }

                        OutputMessage::StopSession => {
                            if let Some(ref mut out) = streaming_output {
                                out.stop();
                            }
                            streaming_output = None;
                            if let Some(h) = ref_fwd_handle.take() {
                                h.abort();
                            }
                            tracing::info!("output node: session stopped (interrupted)");
                        }
                    }
                }
            }
        }
    });

    Handle {
        control_tx: ctl_tx,
        join,
    }
}
