use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::output::StreamingOutput;
use crate::protocol::Event;

use super::{AudioFrame, FlushAck, FlushSignal, NodeId, OutputMessage, RequestId};
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
///   - `msg_rx`:              OutputMessages from TtsNode
///   - `user_speaking_rx`:    watch state from VadNode (for barge-in detection)
///
/// Outputs:
///   - `render_ref_tx`:       speaker reference audio → AecNode
///   - `event_tx`:            IPC events (SpeakDone) → IpcSink
///   - `speaker_active_tx`:   watch state → VAD (and pipeline)
pub fn spawn(
    mut msg_rx: mpsc::Receiver<OutputMessage>,
    render_ref_tx: mpsc::UnboundedSender<AudioFrame>,
    event_tx: mpsc::Sender<Event>,
    speaker_active_tx: Option<Arc<watch::Sender<bool>>>,
    mut user_speaking_rx: Option<watch::Receiver<bool>>,
) -> Handle {
    let (ctl_tx, mut ctl_rx) = mpsc::channel(16);

    let join = tokio::spawn(async move {
        tracing::info!("output node: task started");
        let mut streaming_output: Option<StreamingOutput> = None;
        let mut active_request_id: Option<RequestId> = None;
        // Handle for the render-reference forwarding task
        let mut ref_fwd_handle: Option<JoinHandle<()>> = None;

        // Helper: update speaker_active state
        let set_speaker_active = |active: bool| {
            if let Some(ref tx) = speaker_active_tx {
                let _ = tx.send(active);
            }
        };

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
                            let req_id_str = active_request_id.as_ref().map(|r| r.0.clone());
                            if let Some(ref mut out) = streaming_output {
                                out.stop();
                            }
                            streaming_output = None;
                            if let Some(h) = ref_fwd_handle.take() {
                                h.abort();
                            }
                            let ack_req_id = match &signal {
                                FlushSignal::Flush { request_id } => Some(request_id.clone()),
                                FlushSignal::FlushAll => None,
                            };
                            active_request_id = None;
                            set_speaker_active(false);
                            let _ = event_tx.send(Event::SpeakDone {
                                request_id: req_id_str,
                                reason: "interrupted".to_string(),
                            }).await;
                            let _ = reply.send(FlushAck { node: NodeId::Output, request_id: ack_req_id });
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
                            set_speaker_active(false);
                            tracing::info!("output node: shutdown");
                            break;
                        }
                    }
                }

                // Barge-in: user speaking while output is active
                _ = async {
                    match user_speaking_rx.as_mut() {
                        Some(rx) => rx.changed().await.ok(),
                        None => std::future::pending::<Option<()>>().await,
                    }
                } => {
                    let speaking = user_speaking_rx.as_ref()
                        .map(|rx| *rx.borrow())
                        .unwrap_or(false);
                    let is_active = speaker_active_tx.as_ref()
                        .map(|tx| *tx.borrow())
                        .unwrap_or(false);
                    if speaking && is_active {
                        let req_id_str = active_request_id.as_ref().map(|r| r.0.clone());
                        tracing::info!(?req_id_str, "output node: barge-in detected, stopping playback");
                        if let Some(ref mut out) = streaming_output {
                            out.stop();
                        }
                        streaming_output = None;
                        if let Some(h) = ref_fwd_handle.take() {
                            h.abort();
                        }
                        active_request_id = None;
                        set_speaker_active(false);
                        let _ = event_tx.send(Event::SpeakDone {
                            request_id: req_id_str,
                            reason: "interrupted".to_string(),
                        }).await;
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
                                    set_speaker_active(true);

                                    // Spawn render-reference forwarder
                                    let ref_tx = render_ref_tx.clone();
                                    ref_fwd_handle = Some(tokio::spawn(async move {
                                        while let Some(samples) = ref_rx.recv().await {
                                            let _ = ref_tx.send(AudioFrame {
                                                samples,
                                                sample_rate: output_rate,
                                                vad: None,
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
                                let sa_tx = speaker_active_tx.as_ref().map(Arc::clone);
                                let ev_tx = event_tx.clone();
                                let req_id_str = request_id.0.clone();
                                tokio::spawn(async move {
                                    while playing.load(Ordering::SeqCst) {
                                        tokio::time::sleep(Duration::from_millis(50)).await;
                                    }
                                    if let Some(ref tx) = sa_tx {
                                        let _ = tx.send(false);
                                    }
                                    tracing::info!("output node: buffer drained");
                                    let _ = ev_tx.send(Event::SpeakDone {
                                        request_id: Some(req_id_str),
                                        reason: "completed".to_string(),
                                    }).await;
                                });
                            } else {
                                set_speaker_active(false);
                                let _ = event_tx.send(Event::SpeakDone {
                                    request_id: Some(request_id.0.clone()),
                                    reason: "completed".to_string(),
                                }).await;
                            }

                            // Clean up — keep streaming_output alive for the drain
                            // watcher to read the playing flag; Flush/StopAll will
                            // clear it immediately if a barge-in occurs.
                            active_request_id = None;
                        }

                        OutputMessage::StopSession { request_id } => {
                            if active_request_id.as_ref() != Some(&request_id) {
                                tracing::debug!(?request_id, "output node: ignoring stale StopSession");
                                continue;
                            }
                            let req_id_str = request_id.0.clone();
                            if let Some(ref mut out) = streaming_output {
                                out.stop();
                            }
                            streaming_output = None;
                            active_request_id = None;
                            set_speaker_active(false);
                            if let Some(h) = ref_fwd_handle.take() {
                                h.abort();
                            }
                            let _ = event_tx.send(Event::SpeakDone {
                                request_id: Some(req_id_str),
                                reason: "stopped".to_string(),
                            }).await;
                            tracing::info!("output node: session stopped (interrupted)");
                        }

                        OutputMessage::StopAll => {
                            let req_id_str = active_request_id.as_ref().map(|r| r.0.clone());
                            if let Some(ref mut out) = streaming_output {
                                out.stop();
                            }
                            streaming_output = None;
                            active_request_id = None;
                            set_speaker_active(false);
                            if let Some(h) = ref_fwd_handle.take() {
                                h.abort();
                            }
                            let _ = event_tx.send(Event::SpeakDone {
                                request_id: req_id_str,
                                reason: "stopped".to_string(),
                            }).await;
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
