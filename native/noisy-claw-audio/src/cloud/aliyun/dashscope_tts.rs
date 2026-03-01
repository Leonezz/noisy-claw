use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use uuid::Uuid;

use crate::cloud::traits::{SpeechSynthesizer, SynthesizerConfig};

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

        let task_id = Uuid::new_v4().to_string().replace('-', "");

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

        // Build run-task message
        let mut parameters = json!({
            "voice": config.voice,
            "format": config.format,
            "sample_rate": config.sample_rate,
        });
        if let Some(speed) = config.speed {
            parameters["rate"] = json!(speed);
        }

        let run_task = json!({
            "header": {
                "action": "run-task",
                "task_id": task_id,
                "streaming": "duplex"
            },
            "payload": {
                "task_group": "audio",
                "task": "tts",
                "function": "SpeechSynthesizer",
                "model": config.model,
                "parameters": parameters,
                "input": {}
            }
        });

        ws_write
            .send(Message::Text(run_task.to_string().into()))
            .await
            .context("failed to send run-task")?;

        // Wait for task-started
        loop {
            match ws_read.next().await {
                Some(Ok(Message::Text(ref msg_text))) => {
                    let v: serde_json::Value = serde_json::from_str(msg_text)
                        .context("invalid JSON from DashScope")?;
                    let action = v["header"]["action"].as_str().unwrap_or("");
                    match action {
                        "task-started" => {
                            tracing::info!("DashScope TTS task started: {task_id}");
                            break;
                        }
                        "task-failed" => {
                            let msg = v["header"]["message"]
                                .as_str()
                                .unwrap_or("unknown error");
                            bail!("DashScope TTS task-failed: {msg}");
                        }
                        _ => {
                            tracing::debug!(action, "ignoring pre-start event");
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    bail!("WebSocket closed before task-started");
                }
                Some(Err(e)) => bail!("WebSocket error: {e}"),
                _ => continue,
            }
        }

        // Send continue-task with text input
        let continue_task = json!({
            "header": {
                "action": "continue-task",
                "task_id": task_id,
                "streaming": "duplex"
            },
            "payload": {
                "input": {
                    "text": text
                }
            }
        });
        ws_write
            .send(Message::Text(continue_task.to_string().into()))
            .await
            .context("failed to send continue-task")?;

        // Send finish-task to signal end of input
        let finish_task = json!({
            "header": {
                "action": "finish-task",
                "task_id": task_id,
                "streaming": "duplex"
            },
            "payload": {
                "input": {}
            }
        });
        ws_write
            .send(Message::Text(finish_task.to_string().into()))
            .await
            .context("failed to send finish-task")?;

        // Collect audio binary frames into temp file
        let extension = match config.format.as_str() {
            "mp3" => "mp3",
            "pcm" => "pcm",
            _ => "wav",
        };
        let mut tmpfile = tempfile::Builder::new()
            .prefix("noisy-claw-tts-")
            .suffix(&format!(".{extension}"))
            .tempfile()
            .context("failed to create temp file")?;

        loop {
            match ws_read.next().await {
                Some(Ok(Message::Binary(ref data))) => {
                    tmpfile
                        .write_all(data)
                        .context("failed to write audio data")?;
                }
                Some(Ok(Message::Text(ref msg_text))) => {
                    let v: serde_json::Value = match serde_json::from_str(msg_text) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let action = v["header"]["action"].as_str().unwrap_or("");
                    match action {
                        "result-generated" => {
                            // Intermediate event — continue collecting audio
                        }
                        "task-finished" => {
                            tracing::info!("DashScope TTS task finished");
                            break;
                        }
                        "task-failed" => {
                            let msg = v["header"]["message"]
                                .as_str()
                                .unwrap_or("unknown");
                            bail!("DashScope TTS task failed: {msg}");
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(e)) => bail!("WebSocket error during TTS: {e}"),
                _ => {}
            }
        }

        // Persist temp file (don't let it auto-delete)
        let path = tmpfile.into_temp_path().keep()
            .context("failed to persist temp file")?;
        Ok(path)
    }
}
