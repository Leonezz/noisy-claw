use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::output::StreamingOutput;

use super::{AudioFrame, FlushAck, FlushSignal, NodeId, OutputMessage, OutputNodeEvent, RequestId};
use super::dump;
use tokio::sync::oneshot;

pub enum Control {
    Pause,
    Resume,
    Flush { signal: FlushSignal, reply: oneshot::Sender<FlushAck> },
    Shutdown,
}

pub struct Handle {
    pub control_tx: mpsc::Sender<Control>,
    join: JoinHandle<()>,
}

impl Handle {
    pub async fn pause(&self) {
        let _ = self.control_tx.send(Control::Pause).await;
    }

    pub async fn resume(&self) {
        let _ = self.control_tx.send(Control::Resume).await;
    }

    pub async fn flush(&self, signal: FlushSignal) -> FlushAck {
        let (tx, rx) = oneshot::channel();
        let _ = self.control_tx.send(Control::Flush { signal, reply: tx }).await;
        rx.await.unwrap_or(FlushAck { node: NodeId::Output, request_id: None })
    }

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
        tracing::info!("output node: task started");
        let mut streaming_output: Option<StreamingOutput> = None;
        let mut active_request_id: Option<RequestId> = None;
        // Handle for the render-reference forwarding task
        let mut ref_fwd_handle: Option<JoinHandle<()>> = None;

        loop {
            tokio::select! {
                Some(ctl) = ctl_rx.recv() => {
                    match ctl {
                        Control::Pause => {
                            if let Some(ref out) = streaming_output {
                                out.pause();
                            }
                            tracing::info!("output node: paused");
                        }
                        Control::Resume => {
                            if let Some(ref out) = streaming_output {
                                out.resume();
                            }
                            tracing::info!("output node: resumed");
                        }
                        Control::Flush { signal, reply } => {
                            if let Some(ref mut out) = streaming_output {
                                out.stop();
                            }
                            streaming_output = None;
                            if let Some(h) = ref_fwd_handle.take() {
                                h.abort();
                            }
                            let req_id = match &signal {
                                FlushSignal::Flush { request_id } => Some(request_id.clone()),
                                FlushSignal::FlushAll => None,
                            };
                            active_request_id = None;
                            let _ = reply.send(FlushAck { node: NodeId::Output, request_id: req_id });
                            tracing::info!("output node: flushed");
                        }
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
                        OutputMessage::StartSession { request_id, sample_rate } => {
                            // Clean up previous session if any
                            if let Some(ref mut out) = streaming_output {
                                out.stop();
                            }
                            if let Some(h) = ref_fwd_handle.take() {
                                h.abort();
                            }

                            active_request_id = Some(request_id);
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

                        OutputMessage::AudioChunk { request_id, samples, sample_rate: chunk_sr } => {
                            dump::write("tts_out", &samples, chunk_sr);

                            if active_request_id.as_ref() != Some(&request_id) {
                                tracing::debug!(
                                    ?request_id,
                                    ?active_request_id,
                                    "output node: rejecting stale audio chunk"
                                );
                                continue;
                            }
                            if let Some(ref mut out) = streaming_output {
                                let written = out.write_samples(&samples, chunk_sr);
                                tracing::debug!(
                                    chunk_samples = samples.len(),
                                    written,
                                    "output node: audio chunk written"
                                );
                            }
                        }

                        OutputMessage::FinishSession { request_id } => {
                            if active_request_id.as_ref() != Some(&request_id) {
                                tracing::debug!(?request_id, "output node: ignoring stale FinishSession");
                                continue;
                            }
                            if let Some(ref out) = streaming_output {
                                out.finish();

                                // Spawn a non-blocking drain watcher so the select
                                // loop stays responsive to Flush/Shutdown controls
                                let playing = out.playing_flag();
                                let drain_tx = internal_tx.clone();
                                tokio::spawn(async move {
                                    while playing.load(Ordering::SeqCst) {
                                        tokio::time::sleep(Duration::from_millis(50)).await;
                                    }
                                    tracing::info!("output node: buffer drained");
                                    let _ = drain_tx.send(OutputNodeEvent::SpeakDone).await;
                                });
                            } else {
                                let _ = internal_tx.send(OutputNodeEvent::SpeakDone).await;
                            }

                            // Clean up — keep streaming_output alive for the drain
                            // watcher to read the playing flag; Flush/StopAll will
                            // clear it immediately if a barge-in occurs.
                            active_request_id = None;
                            // Note: streaming_output and ref_fwd_handle are NOT
                            // cleaned up here — they will be cleaned up by the next
                            // StartSession, Flush, StopAll, or Shutdown handler.
                        }

                        OutputMessage::StopSession { request_id } => {
                            if active_request_id.as_ref() != Some(&request_id) {
                                tracing::debug!(?request_id, "output node: ignoring stale StopSession");
                                continue;
                            }
                            if let Some(ref mut out) = streaming_output {
                                out.stop();
                            }
                            streaming_output = None;
                            active_request_id = None;
                            if let Some(h) = ref_fwd_handle.take() {
                                h.abort();
                            }
                            tracing::info!("output node: session stopped (interrupted)");
                        }

                        OutputMessage::StopAll => {
                            if let Some(ref mut out) = streaming_output {
                                out.stop();
                            }
                            streaming_output = None;
                            active_request_id = None;
                            if let Some(h) = ref_fwd_handle.take() {
                                h.abort();
                            }
                            tracing::info!("output node: all sessions stopped");
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
