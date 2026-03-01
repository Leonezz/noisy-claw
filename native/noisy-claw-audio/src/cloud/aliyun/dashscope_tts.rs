use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use std::io::Write;
use std::path::PathBuf;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use uuid::Uuid;

use crate::cloud::aliyun::dashscope_protocol::{
    self, DashScopeEvent, TtsParameters,
};
use crate::audio_utils::pcm_bytes_to_f32;
use crate::cloud::traits::{SpeechSynthesizer, StreamingSpeechSynthesizer, SynthesizerConfig, TtsSession};
use tokio::sync::mpsc;

const DEFAULT_ENDPOINT: &str = "wss://dashscope.aliyuncs.com/api-ws/v1/inference";

pub struct DashScopeSynthesizer;

impl DashScopeSynthesizer {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SpeechSynthesizer for DashScopeSynthesizer {
    async fn synthesize(&self, text: &str, config: &SynthesizerConfig) -> Result<PathBuf> {
        let endpoint = config
            .endpoint
            .as_deref()
            .unwrap_or(DEFAULT_ENDPOINT);

        let task_id = Uuid::new_v4().to_string();

        // Build WebSocket request with auth header
        let mut request = endpoint.into_client_request()
            .context("failed to build WebSocket request")?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", config.api_key)
                .parse()
                .context("invalid API key for header")?,
        );

        let (ws_stream, _) = connect_async(request)
            .await
            .context("WebSocket connection failed")?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        // Build and send run-task message
        let run_task_str = dashscope_protocol::run_task_tts(
            &task_id,
            &config.model,
            TtsParameters {
                voice: config.voice.clone(),
                format: config.format.clone(),
                sample_rate: config.sample_rate,
                rate: config.speed,
            },
        );

        tracing::info!("DashScope TTS sending run-task: {run_task_str}");
        ws_write
            .send(Message::Text(run_task_str.into()))
            .await
            .context("failed to send run-task")?;

        // Wait for task-started
        loop {
            match ws_read.next().await {
                Some(Ok(Message::Text(ref msg_text))) => {
                    tracing::debug!("DashScope TTS response: {msg_text}");
                    match dashscope_protocol::parse_event(msg_text)? {
                        DashScopeEvent::TaskStarted { .. } => {
                            tracing::info!("DashScope TTS task started: {task_id}");
                            break;
                        }
                        DashScopeEvent::TaskFailed { code, message, .. } => {
                            bail!("DashScope TTS task-failed: [{code}] {message}");
                        }
                        other => {
                            tracing::warn!("unexpected TTS pre-start event: {other:?}");
                        }
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    let reason = frame
                        .map(|f| format!("code={}, reason={}", f.code, f.reason))
                        .unwrap_or_else(|| "no close frame".to_string());
                    bail!("WebSocket closed before TTS task-started: {reason}");
                }
                None => {
                    bail!("WebSocket stream ended before TTS task-started");
                }
                Some(Err(e)) => bail!("WebSocket error: {e}"),
                _ => continue,
            }
        }

        // Send continue-task with text input
        tracing::info!(
            text_len = text.len(),
            text = %text.chars().take(80).collect::<String>(),
            "TTS → synthesize text"
        );
        let continue_str = dashscope_protocol::continue_task(&task_id, text);
        ws_write
            .send(Message::Text(continue_str.into()))
            .await
            .context("failed to send continue-task")?;

        // Send finish-task to signal end of input
        let finish_str = dashscope_protocol::finish_task(&task_id);
        ws_write
            .send(Message::Text(finish_str.into()))
            .await
            .context("failed to send finish-task")?;

        // Collect audio binary frames into a file
        let extension = match config.format.as_str() {
            "mp3" => "mp3",
            "pcm" => "pcm",
            _ => "wav",
        };
        let path = std::env::temp_dir()
            .join(format!("noisy-claw-tts-{task_id}.{extension}"));
        let mut file = std::fs::File::create(&path)
            .context("failed to create TTS output file")?;

        loop {
            match ws_read.next().await {
                Some(Ok(Message::Binary(ref data))) => {
                    file.write_all(data)
                        .context("failed to write audio data")?;
                }
                Some(Ok(Message::Text(ref msg_text))) => {
                    let event = match dashscope_protocol::parse_event(msg_text) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };
                    match event {
                        DashScopeEvent::ResultGenerated { .. } => {
                            // Intermediate event — continue collecting audio
                        }
                        DashScopeEvent::TaskFinished { .. } => {
                            tracing::info!("DashScope TTS task finished");
                            break;
                        }
                        DashScopeEvent::TaskFailed { message, .. } => {
                            bail!("DashScope TTS task failed: {message}");
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(e)) => bail!("WebSocket error during TTS: {e}"),
                _ => {}
            }
        }

        // Flush and close before returning the path
        file.flush().context("failed to flush TTS output")?;
        drop(file);
        Ok(path)
    }
}

#[async_trait]
impl StreamingSpeechSynthesizer for DashScopeSynthesizer {
    async fn synthesize_streaming(
        &self,
        text: &str,
        config: &SynthesizerConfig,
        audio_tx: mpsc::Sender<Vec<f32>>,
    ) -> Result<()> {
        let endpoint = config
            .endpoint
            .as_deref()
            .unwrap_or(DEFAULT_ENDPOINT);

        let task_id = Uuid::new_v4().to_string();

        let mut request = endpoint.into_client_request()
            .context("failed to build WebSocket request")?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", config.api_key)
                .parse()
                .context("invalid API key for header")?,
        );

        let (ws_stream, _) = connect_async(request)
            .await
            .context("WebSocket connection failed")?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        // Force PCM format for streaming (no container headers)
        let run_task_str = dashscope_protocol::run_task_tts(
            &task_id,
            &config.model,
            TtsParameters {
                voice: config.voice.clone(),
                format: "pcm".to_string(),
                sample_rate: config.sample_rate,
                rate: config.speed,
            },
        );

        ws_write
            .send(Message::Text(run_task_str.into()))
            .await
            .context("failed to send run-task")?;

        // Wait for task-started
        loop {
            match ws_read.next().await {
                Some(Ok(Message::Text(ref msg_text))) => {
                    match dashscope_protocol::parse_event(msg_text)? {
                        DashScopeEvent::TaskStarted { .. } => {
                            tracing::info!("streaming TTS task started: {task_id}");
                            break;
                        }
                        DashScopeEvent::TaskFailed { code, message, .. } => {
                            bail!("streaming TTS task-failed: [{code}] {message}");
                        }
                        _ => continue,
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    let reason = frame
                        .map(|f| format!("code={}, reason={}", f.code, f.reason))
                        .unwrap_or_else(|| "no close frame".to_string());
                    bail!("WebSocket closed before streaming TTS task-started: {reason}");
                }
                None => bail!("WebSocket ended before streaming TTS task-started"),
                Some(Err(e)) => bail!("WebSocket error: {e}"),
                _ => continue,
            }
        }

        // Send text input + finish
        tracing::info!(
            text_len = text.len(),
            text = %text.chars().take(80).collect::<String>(),
            "TTS → synthesize_streaming text"
        );
        let continue_str = dashscope_protocol::continue_task(&task_id, text);
        ws_write
            .send(Message::Text(continue_str.into()))
            .await
            .context("failed to send continue-task")?;

        let finish_str = dashscope_protocol::finish_task(&task_id);
        ws_write
            .send(Message::Text(finish_str.into()))
            .await
            .context("failed to send finish-task")?;

        // Stream audio chunks to the channel
        loop {
            match ws_read.next().await {
                Some(Ok(Message::Binary(ref data))) => {
                    let samples = pcm_bytes_to_f32(data);
                    tracing::debug!(
                        bytes = data.len(),
                        samples = samples.len(),
                        "streaming TTS audio chunk received"
                    );
                    if audio_tx.send(samples).await.is_err() {
                        tracing::warn!("audio_tx receiver dropped, aborting TTS stream");
                        break;
                    }
                }
                Some(Ok(Message::Text(ref msg_text))) => {
                    let event = match dashscope_protocol::parse_event(msg_text) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };
                    match event {
                        DashScopeEvent::ResultGenerated { .. } => {}
                        DashScopeEvent::TaskFinished { .. } => {
                            tracing::info!("streaming TTS task finished");
                            break;
                        }
                        DashScopeEvent::TaskFailed { message, .. } => {
                            bail!("streaming TTS task failed: {message}");
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(e)) => bail!("WebSocket error during streaming TTS: {e}"),
                _ => {}
            }
        }

        Ok(())
    }
}

/// A TTS session that keeps a WebSocket open for multiple text chunks.
pub struct DashScopeTtsSession {
    ws_write: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    task_id: String,
    reader_handle: tokio::task::JoinHandle<()>,
}

impl DashScopeTtsSession {
    pub async fn start(
        config: &SynthesizerConfig,
        audio_tx: mpsc::Sender<Vec<f32>>,
    ) -> Result<Self> {
        let endpoint = config
            .endpoint
            .as_deref()
            .unwrap_or(DEFAULT_ENDPOINT);

        let task_id = Uuid::new_v4().to_string();

        let mut request = endpoint.into_client_request()
            .context("failed to build WebSocket request")?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", config.api_key)
                .parse()
                .context("invalid API key for header")?,
        );

        let (ws_stream, _) = connect_async(request)
            .await
            .context("WebSocket connection failed")?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        // Force PCM for streaming
        let run_task_str = dashscope_protocol::run_task_tts(
            &task_id,
            &config.model,
            TtsParameters {
                voice: config.voice.clone(),
                format: "pcm".to_string(),
                sample_rate: config.sample_rate,
                rate: config.speed,
            },
        );

        ws_write
            .send(Message::Text(run_task_str.into()))
            .await
            .context("failed to send run-task")?;

        // Wait for task-started
        loop {
            match ws_read.next().await {
                Some(Ok(Message::Text(ref msg_text))) => {
                    match dashscope_protocol::parse_event(msg_text)? {
                        DashScopeEvent::TaskStarted { .. } => {
                            tracing::info!("TTS session started: {task_id}");
                            break;
                        }
                        DashScopeEvent::TaskFailed { code, message, .. } => {
                            bail!("TTS session task-failed: [{code}] {message}");
                        }
                        _ => continue,
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    let reason = frame
                        .map(|f| format!("code={}, reason={}", f.code, f.reason))
                        .unwrap_or_else(|| "no close frame".to_string());
                    bail!("WebSocket closed before TTS session task-started: {reason}");
                }
                None => bail!("WebSocket ended before TTS session task-started"),
                Some(Err(e)) => bail!("WebSocket error: {e}"),
                _ => continue,
            }
        }

        // Spawn reader task to forward audio chunks
        let reader_handle = tokio::spawn(async move {
            loop {
                match ws_read.next().await {
                    Some(Ok(Message::Binary(ref data))) => {
                        let samples = pcm_bytes_to_f32(data);
                        tracing::debug!(
                            bytes = data.len(),
                            samples = samples.len(),
                            "TTS session audio chunk received"
                        );
                        if audio_tx.send(samples).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(ref msg_text))) => {
                        let event = match dashscope_protocol::parse_event(msg_text) {
                            Ok(e) => e,
                            Err(_) => continue,
                        };
                        match event {
                            DashScopeEvent::ResultGenerated { .. } => {}
                            DashScopeEvent::TaskFinished { .. } => {
                                tracing::info!("TTS session task finished");
                                break;
                            }
                            DashScopeEvent::TaskFailed { message, .. } => {
                                tracing::error!("TTS session task failed: {message}");
                                break;
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        tracing::error!("TTS session WebSocket error: {e}");
                        break;
                    }
                    _ => {}
                }
            }
        });

        let tid = task_id.clone();
        Ok(Self {
            ws_write,
            task_id: tid,
            reader_handle,
        })
    }
}

#[async_trait]
impl TtsSession for DashScopeTtsSession {
    async fn send_text(&mut self, text: &str) -> Result<()> {
        tracing::info!(
            text_len = text.len(),
            text = %text.chars().take(80).collect::<String>(),
            "TTS → send_text"
        );
        let msg = dashscope_protocol::continue_task(&self.task_id, text);
        self.ws_write
            .send(Message::Text(msg.into()))
            .await
            .context("failed to send TTS text chunk")?;
        Ok(())
    }

    async fn finish(&mut self) -> Result<()> {
        let msg = dashscope_protocol::finish_task(&self.task_id);
        self.ws_write
            .send(Message::Text(msg.into()))
            .await
            .context("failed to send TTS finish")?;
        // Don't await reader_handle — let it finish independently.
        // When the reader sees TaskFinished it exits and drops audio_tx,
        // closing the channel so the consumer (Branch 5) detects completion.
        Ok(())
    }

    async fn cancel(&mut self) {
        self.reader_handle.abort();
    }
}
