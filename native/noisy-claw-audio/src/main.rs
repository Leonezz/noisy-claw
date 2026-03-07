use noisy_claw_audio::{pipeline, playback, protocol};

use anyhow::Result;
use protocol::{Command, Event};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use pipeline::graph::pipeline::PipelineRequest;
use pipeline::graph::Pipeline;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);
fn next_request_id() -> String {
    let n = REQUEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("req-{n:06}")
}

fn pipeline_definition_json(models_dir: &PathBuf) -> String {
    let vad_model = models_dir.join("silero_vad.onnx").display().to_string();
    let topic_model = models_dir.join("multilingual-MiniLM-L12-v2.onnx").display().to_string();
    let topic_tokenizer = models_dir.join("multilingual-MiniLM-L12-v2-tokenizer.json").display().to_string();

    serde_json::json!({
        "name": "noisy-claw",
        "nodes": [
            { "name": "capture", "type": "capture", "properties": { "device": "default", "sample_rate": 48000 } },
            { "name": "aec",      "type": "aec",      "properties": {} },
            { "name": "vad",      "type": "vad",      "properties": { "model_path": vad_model, "threshold": 0.5 } },
            { "name": "stt_local",  "type": "stt_local",  "properties": {} },
            { "name": "stt_cloud",  "type": "stt_cloud",  "properties": {} },
            { "name": "tts",      "type": "tts",      "properties": {} },
            { "name": "output",   "type": "output",   "properties": {} },
            { "name": "topic",    "type": "topic",    "properties": {
                "model_path": topic_model,
                "tokenizer_path": topic_tokenizer,
                "enabled": false,
            }},
        ],
        "links": [
            // Audio pipeline: capture → aec → vad → stt (both local and cloud)
            { "from": "capture:audio_out",         "to": "aec:capture_in" },
            { "from": "output:render_ref_out",     "to": "aec:render_in" },
            { "from": "aec:audio_out",             "to": "vad:audio_in" },
            { "from": "vad:audio_out",             "to": "stt_local:audio_in" },
            { "from": "vad:audio_out",             "to": "stt_cloud:audio_in" },
            // State: output → vad (speaker active), vad → stt/tts/output (user speaking)
            { "from": "output:speaker_active_out", "to": "vad:speaker_state_in" },
            { "from": "vad:user_speaking_out",     "to": "stt_local:vad_in" },
            { "from": "vad:user_speaking_out",     "to": "stt_cloud:vad_in" },
            { "from": "vad:user_speaking_out",     "to": "tts:user_speaking_in" },
            { "from": "vad:user_speaking_out",     "to": "output:user_speaking_in" },
            // TTS → output
            { "from": "tts:output_msg_out",        "to": "output:output_msg_in" },
            // Cloud STT transcripts → topic detection (meeting mode)
            { "from": "stt_cloud:ipc_event_out",   "to": "topic:transcript_event_in" },
        ],
        "modes": {
            "conversation": {
                "vad": { "threshold": 0.5 },
                "topic": { "enabled": false }
            },
            "meeting": {
                "vad": { "threshold": 0.3 },
                "topic": { "enabled": true }
            },
            "dictation": {
                "vad": { "threshold": 0.3 },
                "topic": { "enabled": false }
            }
        }
    }).to_string()
}

/// Write an event as JSON to stdout.
fn emit_event(event: &Event) {
    if let Ok(json) = serde_json::to_string(event) {
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        let _ = writeln!(stdout, "{}", json);
        let _ = stdout.flush();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_file(true)
        .with_line_number(true)
        .with_target(false)
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "noisy_claw_audio=trace".to_string()),
        )
        .init();

    // ── Audio dump (opt-in via AUDIO_DUMP_DIR env var) ────────────────
    let dump_enabled = pipeline::dump::init();
    tracing::info!(dump_enabled, "audio dump");

    // ── Pipeline introspection channel ─────────────────────────────────
    let (introspect_tx, mut introspect_rx) = mpsc::channel::<PipelineRequest>(32);

    // ── WebSocket audio tap server (opt-in via AUDIO_TAP_PORT env var) ──
    if let Ok(port_str) = std::env::var("AUDIO_TAP_PORT") {
        let port: u16 = port_str.parse().unwrap_or(9876);
        let dump_base = pipeline::dump::dump_base_dir();
        pipeline::tap::spawn_server(port, dump_base, introspect_tx.clone());
    }

    // ── Build and start the pipeline from JSON definition ────────────
    let models_dir = noisy_claw_audio::resolve_models_dir();
    tracing::info!(models_dir = %models_dir.display(), "orchestrator: building pipeline");

    let json = pipeline_definition_json(&models_dir);
    let mut pipe = Pipeline::from_json(&json)?;

    // Take event stream from pipeline (all IPC events auto-collected)
    let event_tx = pipe.event_tx();
    let mut event_rx = pipe.take_event_rx()
        .expect("event_rx should be available before start");

    pipe.start().await?;
    tracing::info!("orchestrator: pipeline started");

    // ── File-based playback (not part of the pipeline) ─────────────────
    let mut playback_engine: Option<playback::AudioPlayback> = None;
    let mut playback_done_rx: Option<tokio::sync::oneshot::Receiver<()>> = None;

    // ── Ready ──────────────────────────────────────────────────────────
    event_tx.send(Event::Ready).ok();

    // ── IPC command loop ─────────────────────────────────────────────
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    loop {
        tokio::select! {
            // ── Commands from stdin ────────────────────────────────────
            line = lines.next_line() => {
                let line = match line? {
                    Some(l) => l,
                    None => break, // EOF
                };
                if line.is_empty() {
                    continue;
                }

                match serde_json::from_str::<Command>(&line) {
                    Ok(cmd) => {
                        let should_exit = handle_command(
                            cmd,
                            &mut pipe,
                            &mut playback_engine,
                            &mut playback_done_rx,
                            &event_tx,
                            &models_dir,
                        ).await;
                        if should_exit {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!(%e, %line, "failed to parse command");
                        event_tx.send(Event::Error {
                            message: format!("invalid command: {e}"),
                        }).ok();
                    }
                }
            }

            // ── Events from pipeline → stdout ────────────────────────
            Some(event) = event_rx.recv() => {
                emit_event(&event);
            }

            // ── File-based playback completion ─────────────────────────
            Ok(()) = async {
                match playback_done_rx.as_mut() {
                    Some(rx) => rx.await.map_err(|_| ()),
                    None => std::future::pending::<std::result::Result<(), ()>>().await,
                }
            } => {
                if let Some(ref pb) = playback_engine {
                    pb.set_done();
                }
                playback_done_rx = None;
                event_tx.send(Event::PlaybackDone).ok();
            }

            // ── Pipeline introspection requests (from tap server) ────
            Some(req) = introspect_rx.recv() => {
                handle_introspect_request(req, &mut pipe).await;
            }
        }
    }

    // ── Shutdown ───────────────────────────────────────────────────────
    pipeline::dump::finish();
    pipe.shutdown().await;

    Ok(())
}

/// Handle introspection requests from the tap server.
async fn handle_introspect_request(req: PipelineRequest, pipe: &mut Pipeline) {
    match req {
        PipelineRequest::GetSnapshot { reply } => {
            let _ = reply.send(pipe.snapshot());
        }
        PipelineRequest::GetDefinition { reply } => {
            let _ = reply.send(pipe.definition().clone());
        }
        PipelineRequest::SetProperty { node, key, value, reply } => {
            let _ = reply.send(pipe.set_property(&node, &key, value));
        }
        PipelineRequest::SetMode { mode, reply } => {
            let _ = reply.send(pipe.set_mode(&mode));
        }
        // Lifecycle commands are only supported in standalone mode
        PipelineRequest::LoadPipeline { reply, .. } => {
            let _ = reply.send(Err(anyhow::anyhow!("not supported in IPC mode")));
        }
        PipelineRequest::StartCapture { reply, .. } => {
            let _ = reply.send(Err(anyhow::anyhow!("not supported in IPC mode")));
        }
        PipelineRequest::StopCapture { reply } => {
            let _ = reply.send(Err(anyhow::anyhow!("not supported in IPC mode")));
        }
        PipelineRequest::SendCommand { node, cmd, args, reply } => {
            let _ = reply.send(pipe.send_command(&node, &cmd, args).await);
        }
        PipelineRequest::GetNodeTypes { reply } => {
            use pipeline::graph::registry::NodeRegistry;
            let types = NodeRegistry::iter()
                .map(|e| pipeline::graph::pipeline::NodeTypeInfo {
                    node_type: e.node_type.to_string(),
                    description: e.description.to_string(),
                    ports: (e.ports)(),
                })
                .collect();
            let _ = reply.send(types);
        }
    }
}

async fn handle_command(
    cmd: Command,
    pipe: &mut Pipeline,
    playback_engine: &mut Option<playback::AudioPlayback>,
    playback_done_rx: &mut Option<tokio::sync::oneshot::Receiver<()>>,
    event_tx: &mpsc::UnboundedSender<Event>,
    models_dir: &PathBuf,
) -> bool {
    tracing::info!(?cmd, "orchestrator: command received");

    match cmd {
        Command::StartCapture { device, stt, .. } => {
            let stt_provider = stt.as_ref()
                .map(|c| c.provider.clone())
                .unwrap_or_else(|| "whisper".to_string());

            tracing::info!(%device, %stt_provider, "orchestrator: starting capture");

            // Configure the appropriate STT node via properties
            if stt_provider == "whisper" {
                let stt_filename = std::env::var("NOISY_CLAW_STT_MODEL")
                    .unwrap_or_else(|_| "ggml-base.bin".to_string());
                let model_path = models_dir.join(&stt_filename);
                pipe.set_property("stt_local", "model_path",
                    serde_json::json!(model_path.to_string_lossy())).ok();
                pipe.set_property("stt_local", "language",
                    serde_json::json!("en")).ok();
            } else if let Some(stt_config) = stt {
                pipe.send_command("stt_cloud", "start_cloud",
                    serde_json::to_value(&stt_config).unwrap_or_default()).await.ok();
            }

            let args = serde_json::json!({ "device": device });
            if let Err(e) = pipe.command("start_capture", args).await {
                tracing::error!(%e, "orchestrator: start_capture failed");
                event_tx.send(Event::Error { message: e.to_string() }).ok();
            }
        }

        Command::StopCapture => {
            pipe.command("stop_capture", serde_json::json!({})).await.ok();
            tracing::info!("orchestrator: capture stopped");
        }

        Command::Speak { text, tts, request_id: cmd_req_id } => {
            let req_id = cmd_req_id.unwrap_or_else(next_request_id);
            pipe.command("speak", serde_json::json!({
                "text": text, "tts": tts, "request_id": req_id,
            })).await.ok();
        }

        Command::SpeakStart { tts, request_id: cmd_req_id } => {
            let req_id = cmd_req_id.unwrap_or_else(next_request_id);
            pipe.command("speak_start", serde_json::json!({
                "tts": tts, "request_id": req_id,
            })).await.ok();
        }

        Command::SpeakChunk { text } => {
            pipe.command("speak_chunk", serde_json::json!({ "text": text })).await.ok();
        }

        Command::SpeakEnd => {
            pipe.command("speak_end", serde_json::json!({})).await.ok();
        }

        Command::FlushSpeak { request_id } => {
            pipe.command("flush_speak", serde_json::json!({ "request_id": request_id })).await.ok();
        }

        Command::StopSpeaking => {
            pipe.command("stop_speaking", serde_json::json!({})).await.ok();
            // Also stop file-based playback
            if let Some(ref mut pb) = playback_engine {
                pb.stop();
            }
            *playback_done_rx = None;
            tracing::info!("orchestrator: speaking stopped");
        }

        Command::PlayAudio { path } => {
            if playback_engine.is_none() {
                match playback::AudioPlayback::new() {
                    Ok(p) => *playback_engine = Some(p),
                    Err(e) => {
                        event_tx.send(Event::Error {
                            message: format!("playback init failed: {e}"),
                        }).ok();
                        return false;
                    }
                }
            }
            let pb = playback_engine.as_mut().unwrap();
            match pb.play(std::path::Path::new(&path)) {
                Ok(player) => {
                    tracing::info!(%path, "orchestrator: playback started");
                    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
                    *playback_done_rx = Some(done_rx);
                    tokio::task::spawn_blocking(move || {
                        player.sleep_until_end();
                        let _ = done_tx.send(());
                    });
                }
                Err(e) => {
                    event_tx.send(Event::Error {
                        message: format!("playback failed: {e}"),
                    }).ok();
                }
            }
        }

        Command::StopPlayback => {
            if let Some(ref mut pb) = playback_engine {
                pb.stop();
            }
            *playback_done_rx = None;
            tracing::info!("orchestrator: playback stopped");
        }

        Command::SetMode { mode } => {
            tracing::info!(%mode, "orchestrator: mode set");
            if let Err(e) = pipe.command("set_mode", serde_json::json!({ "mode": mode })).await {
                event_tx.send(Event::Error { message: e.to_string() }).ok();
            }
        }

        Command::GetStatus => {
            match pipe.command("get_status", serde_json::json!({})).await {
                Ok(status) => {
                    let capturing = status.get("capturing").and_then(|v| v.as_bool()).unwrap_or(false);
                    let speaking = status.get("speaking").and_then(|v| v.as_bool()).unwrap_or(false);
                    event_tx.send(Event::Status {
                        capturing,
                        playing: playback_engine.as_ref().map_or(false, |p| p.is_playing()),
                        speaking,
                    }).ok();
                }
                Err(e) => {
                    tracing::error!(%e, "orchestrator: get_status failed");
                    event_tx.send(Event::Status {
                        capturing: false,
                        playing: playback_engine.as_ref().map_or(false, |p| p.is_playing()),
                        speaking: false,
                    }).ok();
                }
            }
        }

        Command::Shutdown => {
            pipe.command("stop_capture", serde_json::json!({})).await.ok();
            pipe.command("stop_speaking", serde_json::json!({})).await.ok();
            if let Some(ref mut pb) = playback_engine {
                pb.stop();
            }
            tracing::info!("orchestrator: shutdown");
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use noisy_claw_audio::resolve_models_dir;

    #[test]
    fn resolve_models_dir_returns_path() {
        let path = resolve_models_dir();
        assert!(!path.as_os_str().is_empty());
    }

    #[test]
    fn resolve_models_dir_fallback_is_models() {
        let path = resolve_models_dir();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(name, "models");
    }

    #[test]
    fn pipeline_definition_parses() {
        let models = PathBuf::from("models");
        let json = pipeline_definition_json(&models);
        let def: pipeline::graph::PipelineDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(def.name, "noisy-claw");
        assert_eq!(def.nodes.len(), 8);
        assert_eq!(def.links.len(), 12);
        assert_eq!(def.modes.len(), 3);
    }
}
