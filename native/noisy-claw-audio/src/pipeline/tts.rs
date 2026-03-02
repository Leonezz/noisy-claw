use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cloud;
use crate::cloud::traits::SynthesizerConfig;
use crate::protocol::{Event, TtsConfig};

use super::OutputMessage;

pub enum Control {
    /// Synthesize full text in batch mode.
    Speak { text: String, tts_config: TtsConfig },
    /// Begin a streaming TTS session.
    SpeakStart { tts_config: TtsConfig },
    /// Send a text chunk through the active session.
    SpeakChunk { text: String },
    /// Signal end of streaming text input.
    SpeakEnd,
    /// Cancel active synthesis immediately.
    Stop,
    Shutdown,
}

pub struct Handle {
    pub control_tx: mpsc::Sender<Control>,
    join: JoinHandle<()>,
}

impl Handle {
    pub async fn speak(&self, text: String, tts_config: TtsConfig) {
        let _ = self
            .control_tx
            .send(Control::Speak { text, tts_config })
            .await;
    }

    pub async fn speak_start(&self, tts_config: TtsConfig) {
        let _ = self
            .control_tx
            .send(Control::SpeakStart { tts_config })
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

    pub async fn shutdown(self) {
        let _ = self.control_tx.send(Control::Shutdown).await;
        let _ = self.join.await;
    }
}

/// Spawn the TTS node.
///
/// Outputs:
///   - `output_tx`: OutputMessages to the OutputNode
///   - `event_tx`:  IPC events (errors)
pub fn spawn(
    output_tx: mpsc::Sender<OutputMessage>,
    event_tx: mpsc::Sender<Event>,
) -> Handle {
    let (ctl_tx, mut ctl_rx) = mpsc::channel(16);

    let join = tokio::spawn(async move {
        let mut synthesis_handle: Option<JoinHandle<()>> = None;
        let mut tts_session: Option<Box<dyn cloud::traits::TtsSession>> = None;
        // For streaming TTS: task that forwards audio chunks from session → output
        let mut forwarding_handle: Option<JoinHandle<()>> = None;

        loop {
            tokio::select! {
                Some(ctl) = ctl_rx.recv() => {
                    match ctl {
                        Control::Speak { text, tts_config } => {
                            // Cancel any previous synthesis
                            cancel_active(
                                &mut synthesis_handle,
                                &mut tts_session,
                                &mut forwarding_handle,
                            ).await;

                            let sample_rate = tts_config.sample_rate.unwrap_or(16000);
                            let out_tx = output_tx.clone();
                            let ev_tx = event_tx.clone();

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
                                    OutputMessage::StartSession { sample_rate }
                                ).await;

                                match cloud::create_streaming_synthesizer(&provider, &model) {
                                    Ok(synthesizer) => {
                                        let (chunk_tx, mut chunk_rx) =
                                            mpsc::channel::<Vec<f32>>(64);

                                        let out_tx2 = out_tx.clone();
                                        let fwd = tokio::spawn(async move {
                                            while let Some(chunk) = chunk_rx.recv().await {
                                                let _ = out_tx2.send(
                                                    OutputMessage::AudioChunk(chunk)
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
                                let _ = out_tx.send(OutputMessage::FinishSession).await;
                            }));
                        }

                        Control::SpeakStart { tts_config } => {
                            cancel_active(
                                &mut synthesis_handle,
                                &mut tts_session,
                                &mut forwarding_handle,
                            ).await;

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
                                OutputMessage::StartSession { sample_rate }
                            ).await;

                            // Create channel for audio chunks from session
                            let (chunk_tx, mut chunk_rx) =
                                mpsc::channel::<Vec<f32>>(64);

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
                                                OutputMessage::AudioChunk(chunk)
                                            ).await;
                                        }
                                        // All chunks forwarded — signal finish
                                        let _ = out_tx.send(
                                            OutputMessage::FinishSession
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
                                        OutputMessage::StopSession
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
                            tracing::info!("TTS node: stopped");
                        }

                        Control::Shutdown => {
                            cancel_active(
                                &mut synthesis_handle,
                                &mut tts_session,
                                &mut forwarding_handle,
                            ).await;
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
