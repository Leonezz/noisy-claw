mod aec;
mod audio_utils;
mod capture;
mod cloud;
mod embedding;
mod output;
mod pipeline;
mod playback;
mod protocol;
mod stt;
mod vad;

use anyhow::Result;
use protocol::{Command, Event};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use pipeline::graph::nodes::capture_node::CaptureNode;
use pipeline::graph::nodes::ipc_sink_node::IpcSinkNode;
use pipeline::graph::nodes::output_node::OutputNode;
use pipeline::graph::nodes::stt_node::SttNode;
use pipeline::graph::nodes::tts_node::TtsNode;
use pipeline::graph::nodes::vad_node::VadNode;
use pipeline::graph::pipeline::PipelineRequest;
use pipeline::graph::Pipeline;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);
fn next_request_id() -> String {
    let n = REQUEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("req-{n:06}")
}

fn pipeline_definition_json(models_dir: &PathBuf) -> String {
    let vad_model = models_dir.join("silero_vad.onnx").display().to_string();

    // Note: the topic node is NOT included here — it's spawned on demand
    // when entering meeting mode, since it requires optional model files.
    serde_json::json!({
        "name": "noisy-claw",
        "nodes": [
            { "name": "capture", "type": "capture", "properties": { "device": "default", "sample_rate": 48000 } },
            { "name": "aec",      "type": "aec",      "properties": {} },
            { "name": "vad",      "type": "vad",      "properties": { "model_path": vad_model, "threshold": 0.5 } },
            { "name": "stt",      "type": "stt",      "properties": {} },
            { "name": "tts",      "type": "tts",      "properties": {} },
            { "name": "output",   "type": "output",   "properties": {} },
            { "name": "ipc_sink", "type": "ipc_sink", "properties": {} },
        ],
        "links": [
            // Audio pipeline: capture → aec → vad → stt
            { "from": "capture:audio_out",     "to": "aec:capture_in" },
            { "from": "output:render_ref_out", "to": "aec:render_in" },
            { "from": "aec:audio_out",         "to": "vad:audio_in" },
            { "from": "vad:audio_out",         "to": "stt:audio_in" },
            { "from": "vad:vad_event_out",     "to": "stt:vad_in" },
            // TTS → output
            { "from": "tts:output_msg_out",    "to": "output:output_msg_in" },
            // IPC events → ipc_sink (fan-in from all event-producing nodes)
            { "from": "vad:ipc_event_out",     "to": "ipc_sink:event_in" },
            { "from": "stt:ipc_event_out",     "to": "ipc_sink:event_in" },
            { "from": "tts:ipc_event_out",     "to": "ipc_sink:event_in" },
        ],
        "modes": {
            "conversation": {
                "vad": { "threshold": 0.5 }
            },
            "meeting": {
                "vad": { "threshold": 0.3 }
            },
            "dictation": {
                "vad": { "threshold": 0.3 }
            }
        }
    }).to_string()
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
    let models_dir = resolve_models_dir();
    tracing::info!(models_dir = %models_dir.display(), "orchestrator: building pipeline");

    let json = pipeline_definition_json(&models_dir);
    let mut pipe = Pipeline::from_json(&json)?;
    pipe.start().await?;

    tracing::info!("orchestrator: pipeline started");

    // ── Extract orchestrator-facing handles via downcast ──────────────
    let event_tx = pipe.downcast_node_mut::<IpcSinkNode>("ipc_sink")
        .and_then(|n| n.take_event_tx())
        .expect("ipc_sink event_tx");

    let mut transcript_tap_rx = pipe.downcast_node_mut::<IpcSinkNode>("ipc_sink")
        .and_then(|n| n.take_transcript_tap_rx())
        .expect("ipc_sink transcript_tap_rx");

    let mut barge_in_rx = pipe.downcast_node_mut::<VadNode>("vad")
        .and_then(|n| n.take_barge_in_rx())
        .expect("vad barge_in_rx");

    let mut internal_rx = pipe.downcast_node_mut::<OutputNode>("output")
        .and_then(|n| n.take_speak_done_rx())
        .expect("output speak_done_rx");

    // ── File-based playback (not part of the pipeline) ─────────────────
    let mut playback_engine: Option<playback::AudioPlayback> = None;
    let mut playback_done_rx: Option<tokio::sync::oneshot::Receiver<()>> = None;

    // ── Orchestrator state ─────────────────────────────────────────────
    let mut is_speaking_tts = false;
    let mut active_request_id: Option<String> = None;
    let mut current_mode: String = "conversation".to_string();
    let mut topic_handle: Option<pipeline::topic::Handle> = None;

    // ── Ready ──────────────────────────────────────────────────────────
    event_tx.send(Event::Ready).await?;

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
                            &mut is_speaking_tts,
                            &mut active_request_id,
                            &mut current_mode,
                            &mut topic_handle,
                        ).await;
                        if should_exit {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!(%e, %line, "failed to parse command");
                        let _ = event_tx.send(Event::Error {
                            message: format!("invalid command: {e}"),
                        }).await;
                    }
                }
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
                let _ = event_tx.send(Event::PlaybackDone).await;
            }

            // ── VAD barge-in: immediate flush cascade ──────────────────
            Some(()) = barge_in_rx.recv() => {
                if is_speaking_tts {
                    let req_id = active_request_id.take().unwrap_or_default();
                    tracing::info!(%req_id, "orchestrator: barge-in confirmed, flushing");

                    flush_speak(&mut pipe, &req_id).await;

                    is_speaking_tts = false;
                    set_speaking_tts(&mut pipe, false);
                    post_flush_vad_reset(&mut pipe).await;

                    let _ = event_tx.send(Event::SpeakDone {
                        request_id: Some(req_id),
                        reason: "interrupted".to_string(),
                    }).await;
                }
            }

            // ── Transcript tap → topic node (meeting mode only) ────────
            Some(text) = transcript_tap_rx.recv() => {
                if current_mode == "meeting" {
                    if let Some(ref handle) = topic_handle {
                        let _ = handle.control_tx.try_send(
                            pipeline::topic::Control::Transcript { text },
                        );
                    }
                }
            }

            // ── Pipeline introspection requests (from tap server) ────────
            Some(req) = introspect_rx.recv() => {
                handle_introspect_request(req, &mut pipe);
            }

            // ── Output node signals speak done ─────────────────────────
            Some(internal_event) = internal_rx.recv() => {
                match internal_event {
                    pipeline::OutputNodeEvent::SpeakDone => {
                        tracing::info!("orchestrator: speak done (natural)");
                        if is_speaking_tts {
                            let req_id = active_request_id.take();
                            is_speaking_tts = false;
                            set_speaking_tts(&mut pipe, false);
                            post_flush_vad_reset(&mut pipe).await;

                            let _ = event_tx.send(Event::SpeakDone {
                                request_id: req_id,
                                reason: "completed".to_string(),
                            }).await;
                        }
                    }
                }
            }
        }
    }

    // ── Shutdown ───────────────────────────────────────────────────────
    pipeline::dump::finish();
    pipe.shutdown().await;

    Ok(())
}

/// Handle introspection requests from the tap server.
fn handle_introspect_request(req: PipelineRequest, pipe: &mut Pipeline) {
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
    }
}

/// Set VAD's speaking_tts state via the pipeline graph.
fn set_speaking_tts(pipe: &mut Pipeline, speaking: bool) {
    if let Some(vad_node) = pipe.downcast_node::<VadNode>("vad") {
        let _ = vad_node.speaking_tts_tx().send(speaking);
    }
}

/// Flush TTS and output nodes for a specific request.
async fn flush_speak(pipe: &mut Pipeline, req_id: &str) {
    if let Some(tts) = pipe.node_mut("tts") {
        let tts_ack = tts.flush(pipeline::FlushSignal::Flush {
            request_id: req_id.to_string(),
        }).await;
        tracing::info!(node = ?tts_ack.node, "orchestrator: TTS flush ack");
    }
    if let Some(out) = pipe.node_mut("output") {
        let out_ack = out.flush(pipeline::FlushSignal::Flush {
            request_id: req_id.to_string(),
        }).await;
        tracing::info!(node = ?out_ack.node, "orchestrator: Output flush ack");
    }
}

/// Post-flush VAD blanking and reset.
async fn post_flush_vad_reset(pipe: &mut Pipeline) {
    if let Some(vad_node) = pipe.downcast_node::<VadNode>("vad") {
        if let Some(h) = vad_node.inner() {
            h.post_flush_blanking().await;
            h.reset().await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_command(
    cmd: Command,
    pipe: &mut Pipeline,
    playback_engine: &mut Option<playback::AudioPlayback>,
    playback_done_rx: &mut Option<tokio::sync::oneshot::Receiver<()>>,
    event_tx: &mpsc::Sender<Event>,
    models_dir: &PathBuf,
    is_speaking_tts: &mut bool,
    active_request_id: &mut Option<String>,
    current_mode: &mut String,
    topic_handle: &mut Option<pipeline::topic::Handle>,
) -> bool {
    tracing::info!(?cmd, "orchestrator: command received");

    match cmd {
        Command::StartCapture {
            device,
            sample_rate,
            stt,
        } => {
            let pipeline_rate = protocol::PIPELINE_SAMPLE_RATE;
            if sample_rate != pipeline_rate {
                tracing::warn!(
                    requested = sample_rate,
                    using = pipeline_rate,
                    "orchestrator: ignoring requested sample_rate, using pipeline rate"
                );
            }

            // Check VAD initialization
            let vad_ok = pipe.downcast_node::<VadNode>("vad")
                .and_then(|n| n.inner())
                .map_or(false, |h| h.is_initialized());
            if !vad_ok {
                tracing::error!("orchestrator: VAD not initialized, rejecting StartCapture");
                let _ = event_tx.send(Event::Error {
                    message: "VAD init failed: model not available".to_string(),
                }).await;
                return false;
            }

            // Start STT (cloud or local)
            let stt_provider = stt
                .as_ref()
                .map(|c| c.provider.as_str())
                .unwrap_or("whisper");

            let stt_provider_str = stt_provider.to_string();
            tracing::info!(
                %device, sample_rate = pipeline_rate, stt_provider = %stt_provider_str,
                "orchestrator: starting capture pipeline"
            );

            if let Some(stt_node) = pipe.downcast_node::<SttNode>("stt") {
                if let Some(stt_handle) = stt_node.inner() {
                    if stt_provider == "whisper" {
                        let stt_filename = std::env::var("NOISY_CLAW_STT_MODEL")
                            .unwrap_or_else(|_| "ggml-base.bin".to_string());
                        tracing::info!(%stt_filename, "orchestrator: starting local Whisper STT");
                        stt_handle
                            .start_local(models_dir.join(&stt_filename), "en".to_string())
                            .await;
                    } else if let Some(stt_config) = stt {
                        tracing::info!(stt_provider = %stt_provider_str, "orchestrator: starting cloud STT");
                        stt_handle.start_cloud(stt_config).await;
                    }
                }
            }

            // Start capture
            if let Some(cap) = pipe.downcast_node::<CaptureNode>("capture") {
                if let Some(h) = cap.inner() {
                    h.start(&device, pipeline_rate).await;
                    tracing::info!(
                        %device, sample_rate = pipeline_rate,
                        is_capturing = h.is_capturing(),
                        "orchestrator: capture started"
                    );
                }
            }
        }

        Command::StopCapture => {
            if let Some(cap) = pipe.downcast_node::<CaptureNode>("capture") {
                if let Some(h) = cap.inner() { h.stop().await; }
            }
            if let Some(stt_node) = pipe.downcast_node::<SttNode>("stt") {
                if let Some(h) = stt_node.inner() { h.stop().await; }
            }
            tracing::info!("orchestrator: capture stopped");
        }

        Command::Speak { text, tts, request_id: cmd_req_id } => {
            let req_id = cmd_req_id.unwrap_or_else(next_request_id);
            *active_request_id = Some(req_id.clone());
            *is_speaking_tts = true;
            set_speaking_tts(pipe, true);
            pipe.set_property("vad", "threshold", serde_json::json!(0.85)).ok();
            let _ = event_tx.send(Event::SpeakStarted {
                request_id: Some(req_id.clone()),
            }).await;

            if let Some(tts_node) = pipe.downcast_node::<TtsNode>("tts") {
                if let Some(h) = tts_node.inner() {
                    h.speak(text, tts, pipeline::RequestId(req_id)).await;
                }
            }
        }

        Command::SpeakStart { tts, request_id: cmd_req_id } => {
            let req_id = cmd_req_id.unwrap_or_else(next_request_id);
            *active_request_id = Some(req_id.clone());
            *is_speaking_tts = true;
            set_speaking_tts(pipe, true);
            pipe.set_property("vad", "threshold", serde_json::json!(0.85)).ok();
            let _ = event_tx.send(Event::SpeakStarted {
                request_id: Some(req_id.clone()),
            }).await;

            if let Some(tts_node) = pipe.downcast_node::<TtsNode>("tts") {
                if let Some(h) = tts_node.inner() {
                    h.speak_start(tts, pipeline::RequestId(req_id)).await;
                }
            }
        }

        Command::SpeakChunk { text } => {
            if let Some(tts_node) = pipe.downcast_node::<TtsNode>("tts") {
                if let Some(h) = tts_node.inner() { h.speak_chunk(text).await; }
            }
        }

        Command::SpeakEnd => {
            if let Some(tts_node) = pipe.downcast_node::<TtsNode>("tts") {
                if let Some(h) = tts_node.inner() { h.speak_end().await; }
            }
        }

        Command::FlushSpeak { request_id } => {
            if *is_speaking_tts {
                flush_speak(pipe, &request_id).await;
                *is_speaking_tts = false;
                set_speaking_tts(pipe, false);
                post_flush_vad_reset(pipe).await;
                let _ = event_tx.send(Event::SpeakDone {
                    request_id: Some(request_id),
                    reason: "interrupted".to_string(),
                }).await;
                *active_request_id = None;
            }
        }

        Command::StopSpeaking => {
            if *is_speaking_tts {
                let req_id = active_request_id.take();
                if let Some(tts_node) = pipe.node_mut("tts") {
                    let _ = tts_node.flush(pipeline::FlushSignal::FlushAll).await;
                }
                if let Some(out_node) = pipe.node_mut("output") {
                    let _ = out_node.flush(pipeline::FlushSignal::FlushAll).await;
                }
                *is_speaking_tts = false;
                set_speaking_tts(pipe, false);
                post_flush_vad_reset(pipe).await;
                let _ = event_tx.send(Event::SpeakDone {
                    request_id: req_id,
                    reason: "stopped".to_string(),
                }).await;
            }
            // Also stop file-based playback if active
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
                        let _ = event_tx
                            .send(Event::Error {
                                message: format!("playback init failed: {e}"),
                            })
                            .await;
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
                    let _ = event_tx
                        .send(Event::Error {
                            message: format!("playback failed: {e}"),
                        })
                        .await;
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

            // Spawn topic node when entering meeting mode
            if mode == "meeting" && topic_handle.is_none() {
                let model_path = models_dir.join("multilingual-MiniLM-L12-v2.onnx");
                let tokenizer_path = models_dir.join("multilingual-MiniLM-L12-v2-tokenizer.json");
                if model_path.exists() && tokenizer_path.exists() {
                    let handle = pipeline::topic::spawn(
                        event_tx.clone(),
                        model_path,
                        tokenizer_path,
                        0.65,   // similarity threshold
                        300.0,  // max block secs
                        30.0,   // silence block secs
                    );
                    *topic_handle = Some(handle);
                    tracing::info!("orchestrator: topic detection node spawned");
                } else {
                    tracing::warn!(
                        model = %model_path.display(),
                        tokenizer = %tokenizer_path.display(),
                        "orchestrator: embedding models not found, topic detection disabled"
                    );
                }
            }

            // Shutdown topic node when leaving meeting mode
            if mode != "meeting" {
                if let Some(handle) = topic_handle.take() {
                    tokio::spawn(async move { handle.shutdown().await });
                    tracing::info!("orchestrator: topic detection node shut down");
                }
            }

            *current_mode = mode;
        }

        Command::GetStatus => {
            let capturing = pipe.downcast_node::<CaptureNode>("capture")
                .and_then(|n| n.inner())
                .map_or(false, |h| h.is_capturing());
            let _ = event_tx
                .send(Event::Status {
                    capturing,
                    playing: playback_engine
                        .as_ref()
                        .map_or(false, |p| p.is_playing()),
                    speaking: *is_speaking_tts,
                })
                .await;
        }

        Command::Shutdown => {
            // Stop active sub-systems before full pipeline shutdown
            if let Some(cap) = pipe.downcast_node::<CaptureNode>("capture") {
                if let Some(h) = cap.inner() { h.stop().await; }
            }
            if let Some(stt_node) = pipe.downcast_node::<SttNode>("stt") {
                if let Some(h) = stt_node.inner() { h.stop().await; }
            }
            if let Some(tts_node) = pipe.downcast_node::<TtsNode>("tts") {
                if let Some(h) = tts_node.inner() { h.stop().await; }
            }
            if let Some(handle) = topic_handle.take() {
                handle.shutdown().await;
            }
            if let Some(ref mut pb) = playback_engine {
                pb.stop();
            }
            tracing::info!("orchestrator: shutdown");
            return true;
        }
    }

    false
}

fn resolve_models_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NOISY_CLAW_MODELS_DIR") {
        let p = PathBuf::from(&dir);
        if p.exists() {
            return p;
        }
        tracing::warn!(
            path = %dir,
            "NOISY_CLAW_MODELS_DIR set but path does not exist, falling back"
        );
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));

    if let Some(ref dir) = exe_dir {
        let models = dir.join("models");
        if models.exists() {
            return models;
        }
    }

    PathBuf::from("models")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(def.nodes.len(), 7); // topic node spawned on demand, not in graph
        assert_eq!(def.links.len(), 9);
        assert_eq!(def.modes.len(), 3);
    }
}
