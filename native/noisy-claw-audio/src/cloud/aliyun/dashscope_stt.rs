use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use uuid::Uuid;

use crate::cloud::aliyun::dashscope_protocol::{
    self, AsrParameters, DashScopeEvent,
};
use crate::cloud::traits::{RecognitionResult, RecognizerConfig, SpeechRecognizer};

const DEFAULT_ENDPOINT: &str = "wss://dashscope.aliyuncs.com/api-ws/v1/inference";

pub struct DashScopeRecognizer {
    audio_tx: Option<mpsc::Sender<Vec<u8>>>,
    result_rx: Option<mpsc::Receiver<RecognitionResult>>,
    stop_tx: Option<mpsc::Sender<()>>,
    ws_handle: Option<tokio::task::JoinHandle<()>>,
}

impl DashScopeRecognizer {
    pub fn new() -> Self {
        Self {
            audio_tx: None,
            result_rx: None,
            stop_tx: None,
            ws_handle: None,
        }
    }
}

/// Convert f32 samples to PCM i16 little-endian bytes.
fn samples_to_pcm_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let i16_val = (clamped * 32767.0) as i16;
        bytes.extend_from_slice(&i16_val.to_le_bytes());
    }
    bytes
}

#[async_trait]
impl SpeechRecognizer for DashScopeRecognizer {
    async fn start(&mut self, config: &RecognizerConfig) -> Result<()> {
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
        let run_task_str = dashscope_protocol::run_task_asr(
            &task_id,
            &config.model,
            AsrParameters {
                format: "pcm".to_string(),
                sample_rate: config.sample_rate,
                language_hints: config.languages.clone(),
                disfluency_removal_enabled: true,
                semantic_punctuation_enabled: true,
                punctuation_prediction_enabled: true,
                max_sentence_silence: 800,
                multi_threshold_mode_enabled: true,
                heartbeat: true,
            },
        );

        tracing::info!("DashScope STT sending run-task: {run_task_str}");
        ws_write
            .send(Message::Text(run_task_str.into()))
            .await
            .context("failed to send run-task")?;

        // Wait for task-started event
        loop {
            match ws_read.next().await {
                Some(Ok(Message::Text(ref text))) => {
                    tracing::debug!("DashScope response: {text}");
                    match dashscope_protocol::parse_event(text)? {
                        DashScopeEvent::TaskStarted { .. } => {
                            tracing::info!("DashScope STT task started: {task_id}");
                            break;
                        }
                        DashScopeEvent::TaskFailed { code, message, .. } => {
                            bail!("DashScope STT task-failed: [{code}] {message}");
                        }
                        other => {
                            tracing::warn!("unexpected pre-start event: {other:?}");
                        }
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    let reason = frame
                        .map(|f| format!("code={}, reason={}", f.code, f.reason))
                        .unwrap_or_else(|| "no close frame".to_string());
                    bail!("WebSocket closed before task-started: {reason}");
                }
                None => {
                    bail!("WebSocket stream ended before task-started");
                }
                Some(Err(e)) => bail!("WebSocket error: {e}"),
                _ => continue,
            }
        }

        // Channels for communication
        let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(64);
        let (result_tx, result_rx) = mpsc::channel::<RecognitionResult>(64);
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);

        let task_id_clone = task_id.clone();

        // Spawn background task to manage WebSocket I/O
        let handle = tokio::spawn(async move {
            let mut stopped = false;

            loop {
                tokio::select! {
                    // Send audio data
                    Some(pcm_bytes) = audio_rx.recv() => {
                        if ws_write.send(Message::Binary(pcm_bytes.into())).await.is_err() {
                            tracing::error!("failed to send audio frame");
                            break;
                        }
                    }

                    // Receive results from server
                    Some(msg) = ws_read.next() => {
                        match msg {
                            Ok(Message::Text(ref text)) => {
                                let event = match dashscope_protocol::parse_event(text) {
                                    Ok(e) => e,
                                    Err(_) => continue,
                                };
                                match event {
                                    DashScopeEvent::ResultGenerated { .. } => {
                                        if let Some(sentence) = event.as_asr_sentence() {
                                            if !sentence.text.is_empty() {
                                                tracing::info!(
                                                    text = %sentence.text,
                                                    is_final = sentence.sentence_end,
                                                    begin_ms = sentence.begin_time,
                                                    end_ms = sentence.end_time,
                                                    "STT ← transcript"
                                                );
                                                let result = RecognitionResult {
                                                    text: sentence.text,
                                                    is_final: sentence.sentence_end,
                                                    start_time: sentence.begin_time / 1000.0,
                                                    end_time: sentence.end_time / 1000.0,
                                                    confidence: None,
                                                };
                                                let _ = result_tx.send(result).await;
                                            }
                                        }
                                    }
                                    DashScopeEvent::TaskFinished { .. } => {
                                        tracing::info!("DashScope STT task finished");
                                        break;
                                    }
                                    DashScopeEvent::TaskFailed { message, .. } => {
                                        tracing::error!("DashScope STT task failed: {message}");
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            Ok(Message::Close(_)) | Err(_) => break,
                            _ => {}
                        }
                    }

                    // Stop signal
                    Some(()) = stop_rx.recv(), if !stopped => {
                        stopped = true;
                        let finish_str = dashscope_protocol::finish_task(&task_id_clone);
                        let _ = ws_write.send(Message::Text(finish_str.into())).await;
                        // Keep reading until task-finished
                    }

                    else => break,
                }
            }
        });

        self.audio_tx = Some(audio_tx);
        self.result_rx = Some(result_rx);
        self.stop_tx = Some(stop_tx);
        self.ws_handle = Some(handle);

        Ok(())
    }

    async fn feed_audio(&mut self, samples: &[f32]) -> Result<()> {
        if let Some(ref tx) = self.audio_tx {
            let pcm = samples_to_pcm_bytes(samples);
            tx.send(pcm)
                .await
                .context("audio channel closed")?;
        }
        Ok(())
    }

    async fn poll_result(&mut self) -> Result<Option<RecognitionResult>> {
        if let Some(ref mut rx) = self.result_rx {
            match rx.try_recv() {
                Ok(result) => Ok(Some(result)),
                Err(mpsc::error::TryRecvError::Empty) => Ok(None),
                Err(mpsc::error::TryRecvError::Disconnected) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(()).await;
        }
        // Drop audio sender to signal no more data
        self.audio_tx = None;

        if let Some(handle) = self.ws_handle.take() {
            // Give it time to finish gracefully
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                handle,
            )
            .await;
        }
        self.result_rx = None;
        Ok(())
    }
}
