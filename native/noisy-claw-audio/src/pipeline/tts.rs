use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::cloud;
use crate::cloud::traits::SynthesizerConfig;
use crate::protocol::{Event, TtsConfig};

use super::{FlushAck, FlushSignal, NodeId, OutputMessage, RequestId};
use tokio::sync::oneshot;

pub enum Control {
    /// Synthesize full text in batch mode.
    Speak { text: String, tts_config: TtsConfig, request_id: RequestId },
    /// Begin a streaming TTS session.
    SpeakStart { tts_config: TtsConfig, request_id: RequestId },
    /// Send a text chunk through the active session.
    SpeakChunk { text: String },
    /// Signal end of streaming text input.
    SpeakEnd,
    /// Cancel active synthesis immediately.
    Stop,
    /// Flush: cancel active synthesis and reply with FlushAck.
    Flush { signal: FlushSignal, reply: oneshot::Sender<FlushAck> },
    Shutdown,
}

pub struct Handle {
    pub control_tx: mpsc::Sender<Control>,
    join: JoinHandle<()>,
}

impl Handle {
    pub async fn speak(&self, text: String, tts_config: TtsConfig, request_id: RequestId) {
        let _ = self
            .control_tx
            .send(Control::Speak { text, tts_config, request_id })
            .await;
    }

    pub async fn speak_start(&self, tts_config: TtsConfig, request_id: RequestId) {
        let _ = self
            .control_tx
            .send(Control::SpeakStart { tts_config, request_id })
            .await;
    }

    pub async fn speak_chunk(&self, text: String) {
        let _ = self.control_tx.send(Control::SpeakChunk { text }).await;
    }

    pub async fn speak_end(&self) {
        let _ = self.control_tx.send(Control::SpeakEnd).await;
    }

    pub async fn stop(&self) {
        let _ = self.control_tx.send(Control::Stop).await;
    }

    pub async fn flush(&self, signal: FlushSignal) -> FlushAck {
        let (tx, rx) = oneshot::channel();
        let _ = self.control_tx.send(Control::Flush { signal, reply: tx }).await;
        rx.await.unwrap_or(FlushAck { node: NodeId::Tts, request_id: None })
    }

    pub async fn shutdown(self) {
        let _ = self.control_tx.send(Control::Shutdown).await;
        let _ = self.join.await;
    }
}

/// Spawn the TTS node.
///
/// Inputs:
///   - `user_speaking_rx`: watch state from VadNode (for barge-in detection)
///
/// Outputs:
///   - `output_tx`: OutputMessages to the OutputNode
///   - `event_tx`:  IPC events (SpeakStarted, errors)
pub fn spawn(
    output_tx: mpsc::Sender<OutputMessage>,
    event_tx: mpsc::Sender<Event>,
    mut user_speaking_rx: Option<watch::Receiver<bool>>,
) -> Handle {
    let (ctl_tx, mut ctl_rx) = mpsc::channel(16);

    let join = tokio::spawn(async move {
        tracing::info!("TTS node: task started");
        let mut synthesis_handle: Option<JoinHandle<()>> = None;
        let mut tts_session: Option<Box<dyn cloud::traits::TtsSession>> = None;
        // For streaming TTS: task that forwards audio chunks from session → output
        let mut forwarding_handle: Option<JoinHandle<()>> = None;
        let mut active_request_id: Option<RequestId> = None;

        loop {
            tokio::select! {
                // Barge-in: user started speaking while TTS is active
                _ = async {
                    match user_speaking_rx.as_mut() {
                        Some(rx) => rx.changed().await.ok(),
                        None => std::future::pending::<Option<()>>().await,
                    }
                } => {
                    let speaking = user_speaking_rx.as_ref()
                        .map(|rx| *rx.borrow())
                        .unwrap_or(false);
                    if speaking && active_request_id.is_some() {
                        let req_id = active_request_id.take();
                        tracing::info!(?req_id, "TTS node: barge-in detected, cancelling synthesis");
                        cancel_active(
                            &mut synthesis_handle,
                            &mut tts_session,
                            &mut forwarding_handle,
                        ).await;
                        // Tell output to stop playing (propagates through data edge)
                        let _ = output_tx.send(OutputMessage::StopAll).await;
                    }
                }

                Some(ctl) = ctl_rx.recv() => {
                    match ctl {
                        Control::Speak { text, tts_config, request_id } => {
                            // Cancel any previous synthesis
                            cancel_active(
                                &mut synthesis_handle,
                                &mut tts_session,
                                &mut forwarding_handle,
                            ).await;

                            active_request_id = Some(request_id.clone());
                            let _ = event_tx.send(Event::SpeakStarted {
                                request_id: Some(request_id.0.clone()),
                            }).await;
                            let sample_rate = tts_config.sample_rate.unwrap_or(16000);
                            let out_tx = output_tx.clone();
                            let ev_tx = event_tx.clone();
                            let req_id = request_id.clone();

                            let model = tts_config.model.as_deref()
                                .unwrap_or("cosyvoice-v3-flash").to_string();
                            let provider = tts_config.provider.clone();
                            let api_key = match &tts_config.api_key {
                                Some(k) => k.clone(),
                                None => {
                                    let _ = ev_tx.send(Event::Error {
                                        message: "TTS requires api_key".to_string(),
                                    }).await;
                                    continue;
                                }
                            };

                            let synth_config = build_synth_config(
                                &api_key, &tts_config, &model, sample_rate,
                            );

                            synthesis_handle = Some(tokio::spawn(async move {
                                // Signal output to start a new session
                                let _ = out_tx.send(
                                    OutputMessage::StartSession { request_id: req_id.clone(), sample_rate }
                                ).await;

                                match cloud::create_streaming_synthesizer(&provider, &model) {
                                    Ok(synthesizer) => {
                                        let (chunk_tx, mut chunk_rx) =
                                            mpsc::channel::<Vec<f32>>(64);

                                        let out_tx2 = out_tx.clone();
                                        let req_id_fwd = req_id.clone();
                                        let fwd = tokio::spawn(async move {
                                            while let Some(chunk) = chunk_rx.recv().await {
                                                let _ = out_tx2.send(
                                                    OutputMessage::AudioChunk {
                                                        request_id: req_id_fwd.clone(),
                                                        samples: chunk,
                                                        sample_rate,
                                                    }
                                                ).await;
                                            }
                                        });

                                        if let Err(e) = synthesizer
                                            .synthesize_streaming(
                                                &text, &synth_config, chunk_tx,
                                            )
                                            .await
                                        {
                                            tracing::error!(
                                                %e,
                                                "TTS node: batch synthesis failed"
                                            );
                                            let _ = ev_tx.send(Event::Error {
                                                message: format!(
                                                    "TTS synthesis failed: {e}"
                                                ),
                                            }).await;
                                        }

                                        // Wait for all chunks to be forwarded
                                        let _ = fwd.await;
                                    }
                                    Err(e) => {
                                        tracing::error!(%e, "TTS node: synthesizer init failed");
                                        let _ = ev_tx.send(Event::Error {
                                            message: format!("TTS init failed: {e}"),
                                        }).await;
                                    }
                                }

                                // Signal output that all audio has been sent
                                let _ = out_tx.send(OutputMessage::FinishSession { request_id: req_id }).await;
                            }));
                        }

                        Control::SpeakStart { tts_config, request_id } => {
                            cancel_active(
                                &mut synthesis_handle,
                                &mut tts_session,
                                &mut forwarding_handle,
                            ).await;

                            active_request_id = Some(request_id.clone());
                            let _ = event_tx.send(Event::SpeakStarted {
                                request_id: Some(request_id.0.clone()),
                            }).await;
                            let sample_rate = tts_config.sample_rate.unwrap_or(16000);
                            let api_key = match &tts_config.api_key {
                                Some(k) => k.clone(),
                                None => {
                                    let _ = event_tx.send(Event::Error {
                                        message: "TTS requires api_key".to_string(),
                                    }).await;
                                    continue;
                                }
                            };

                            let model = tts_config.model.as_deref()
                                .unwrap_or("cosyvoice-v3-flash").to_string();
                            let synth_config = build_synth_config(
                                &api_key, &tts_config, &model, sample_rate,
                            );

                            // Signal output to start session
                            let _ = output_tx.send(
                                OutputMessage::StartSession { request_id: request_id.clone(), sample_rate }
                            ).await;

                            // Create channel for audio chunks from session
                            let (chunk_tx, mut chunk_rx) =
                                mpsc::channel::<Vec<f32>>(64);

                            let req_id_fwd = request_id.clone();
                            match cloud::aliyun::dashscope_tts::DashScopeTtsSession::start(
                                &synth_config, chunk_tx,
                            ).await {
                                Ok(session) => {
                                    tts_session = Some(Box::new(session));
                                    tracing::info!("TTS node: streaming session started");

                                    // Spawn forwarding task
                                    let out_tx = output_tx.clone();
                                    forwarding_handle = Some(tokio::spawn(async move {
                                        while let Some(chunk) = chunk_rx.recv().await {
                                            let _ = out_tx.send(
                                                OutputMessage::AudioChunk {
                                                    request_id: req_id_fwd.clone(),
                                                    samples: chunk,
                                                    sample_rate,
                                                }
                                            ).await;
                                        }
                                        // All chunks forwarded — signal finish
                                        let _ = out_tx.send(
                                            OutputMessage::FinishSession { request_id: req_id_fwd }
                                        ).await;
                                    }));
                                }
                                Err(e) => {
                                    tracing::error!(
                                        %e, "TTS node: session start failed"
                                    );
                                    let _ = event_tx.send(Event::Error {
                                        message: format!(
                                            "TTS session start failed: {e}"
                                        ),
                                    }).await;
                                    // Clean up: no session, tell output to abort
                                    let _ = output_tx.send(
                                        OutputMessage::StopAll
                                    ).await;
                                }
                            }
                        }

                        Control::SpeakChunk { text } => {
                            tracing::info!(
                                text_len = text.len(),
                                text_preview = %text.chars().take(60).collect::<String>(),
                                "TTS node: SpeakChunk"
                            );
                            if let Some(ref mut session) = tts_session {
                                if let Err(e) = session.send_text(&text).await {
                                    tracing::error!(
                                        %e, "TTS node: session send_text failed"
                                    );
                                }
                            } else {
                                tracing::warn!(
                                    "TTS node: SpeakChunk but no active session"
                                );
                            }
                        }

                        Control::SpeakEnd => {
                            tracing::info!("TTS node: SpeakEnd");
                            if let Some(ref mut session) = tts_session {
                                if let Err(e) = session.finish().await {
                                    tracing::error!(
                                        %e, "TTS node: session finish failed"
                                    );
                                }
                            }
                            tts_session = None;
                            // forwarding_handle continues until chunk_rx closes
                        }

                        Control::Stop => {
                            cancel_active(
                                &mut synthesis_handle,
                                &mut tts_session,
                                &mut forwarding_handle,
                            ).await;
                            active_request_id = None;
                            // Notify output node to stop — aborted synthesis may
                            // have sent StartSession without a matching FinishSession
                            let _ = output_tx.send(OutputMessage::StopAll).await;
                            tracing::info!("TTS node: stopped");
                        }

                        Control::Flush { signal, reply } => {
                            cancel_active(
                                &mut synthesis_handle,
                                &mut tts_session,
                                &mut forwarding_handle,
                            ).await;
                            let req_id = match &signal {
                                FlushSignal::Flush { request_id } => Some(request_id.clone()),
                                FlushSignal::FlushAll => None,
                            };
                            active_request_id = None;
                            let _ = reply.send(FlushAck { node: NodeId::Tts, request_id: req_id });
                            tracing::info!("TTS node: flushed");
                        }

                        Control::Shutdown => {
                            cancel_active(
                                &mut synthesis_handle,
                                &mut tts_session,
                                &mut forwarding_handle,
                            ).await;
                            active_request_id = None;
                            tracing::info!("TTS node: shutdown");
                            break;
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

async fn cancel_active(
    synthesis_handle: &mut Option<JoinHandle<()>>,
    tts_session: &mut Option<Box<dyn cloud::traits::TtsSession>>,
    forwarding_handle: &mut Option<JoinHandle<()>>,
) {
    if let Some(handle) = synthesis_handle.take() {
        handle.abort();
    }
    if let Some(ref mut session) = tts_session {
        session.cancel().await;
    }
    *tts_session = None;
    if let Some(handle) = forwarding_handle.take() {
        handle.abort();
    }
}

fn build_synth_config(
    api_key: &str,
    tts_config: &TtsConfig,
    model: &str,
    sample_rate: u32,
) -> SynthesizerConfig {
    SynthesizerConfig {
        api_key: api_key.to_string(),
        endpoint: tts_config.endpoint.clone(),
        model: model.to_string(),
        voice: tts_config
            .voice
            .clone()
            .unwrap_or_else(|| "longanyang".to_string()),
        format: "pcm".to_string(),
        sample_rate,
        speed: tts_config.speed,
        extra: tts_config.extra.clone().unwrap_or_default(),
    }
}
