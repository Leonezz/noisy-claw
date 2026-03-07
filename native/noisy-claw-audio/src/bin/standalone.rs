use anyhow::Result;
use std::path::PathBuf;
use tokio::sync::mpsc;

use noisy_claw_audio::{pipeline, protocol, resolve_models_dir};
use pipeline::graph::pipeline::PipelineRequest;
use pipeline::graph::Pipeline;

fn default_pipeline_json(models_dir: &PathBuf) -> String {
    let vad_model = models_dir.join("silero_vad.onnx").display().to_string();
    let stt_filename = std::env::var("NOISY_CLAW_STT_MODEL")
        .unwrap_or_else(|_| "ggml-base.bin".to_string());
    let stt_model = models_dir.join(&stt_filename).display().to_string();

    // Note: AEC is omitted because it requires a render reference input
    // (speaker audio for echo cancellation), which standalone mode doesn't have.
    serde_json::json!({
        "name": "standalone",
        "nodes": [
            { "name": "capture", "type": "capture",   "properties": { "device": "default", "sample_rate": 48000 } },
            { "name": "vad",     "type": "vad",       "properties": { "model_path": vad_model, "threshold": 0.5 } },
            { "name": "stt",     "type": "stt_local", "properties": { "model_path": stt_model, "language": "en" } },
        ],
        "links": [
            { "from": "capture:audio_out",     "to": "vad:audio_in" },
            { "from": "vad:audio_out",         "to": "stt:audio_in" },
            { "from": "vad:user_speaking_out", "to": "stt:vad_in" },
        ],
        "modes": {
            "conversation": { "vad": { "threshold": 0.5 } },
            "meeting":      { "vad": { "threshold": 0.3 } },
        }
    }).to_string()
}

fn parse_args() -> (u16, Option<String>, bool) {
    let args: Vec<String> = std::env::args().collect();
    let mut port = 9876u16;
    let mut pipeline_file: Option<String> = None;
    let mut no_capture = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" | "-p" => {
                i += 1;
                if i < args.len() {
                    port = args[i].parse().unwrap_or(9876);
                }
            }
            "--pipeline" => {
                i += 1;
                if i < args.len() {
                    pipeline_file = Some(args[i].clone());
                }
            }
            "--no-capture" => {
                no_capture = true;
            }
            _ => {}
        }
        i += 1;
    }
    (port, pipeline_file, no_capture)
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

    let (port, pipeline_file, no_capture) = parse_args();

    // ── Audio dump (opt-in via AUDIO_DUMP_DIR env var) ────────────────
    let dump_enabled = pipeline::dump::init();
    tracing::info!(dump_enabled, "audio dump");

    // ── Pipeline introspection channel ─────────────────────────────────
    let (introspect_tx, mut introspect_rx) = mpsc::channel::<PipelineRequest>(32);

    // ── WebSocket tap server (always on in standalone mode) ────────────
    let dump_base = pipeline::dump::dump_base_dir();
    pipeline::tap::spawn_server(port, dump_base, introspect_tx.clone());
    tracing::info!(%port, "standalone: tap server listening");

    // ── Build and start the pipeline ──────────────────────────────────
    let models_dir = resolve_models_dir();
    let json = if let Some(ref file) = pipeline_file {
        std::fs::read_to_string(file)?
    } else {
        default_pipeline_json(&models_dir)
    };

    let mut pipe = Pipeline::from_json(&json)?;
    // Take event rx (we log events in standalone mode, no stdout IPC)
    let mut event_rx = pipe.take_event_rx();
    pipe.start().await?;
    tracing::info!("standalone: pipeline started");

    // ── Auto-start capture unless --no-capture ────────────────────────
    if !no_capture {
        do_start_capture(&mut pipe, "default").await;
    }

    // ── Main loop ─────────────────────────────────────────────────────
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            Some(req) = introspect_rx.recv() => {
                handle_request(req, &mut pipe).await;
            }
            Some(event) = async {
                match event_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending::<Option<protocol::Event>>().await,
                }
            } => {
                tracing::debug!(?event, "standalone: pipeline event");
            }
            _ = &mut shutdown => {
                tracing::info!("standalone: Ctrl+C received, shutting down");
                break;
            }
        }
    }

    // ── Shutdown ──────────────────────────────────────────────────────
    pipeline::dump::finish();
    pipe.shutdown().await;
    tracing::info!("standalone: clean shutdown complete");

    Ok(())
}

async fn handle_request(req: PipelineRequest, pipe: &mut Pipeline) {
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
        PipelineRequest::LoadPipeline { json, reply } => {
            let result = reload_pipeline(pipe, &json).await;
            let _ = reply.send(result);
        }
        PipelineRequest::StartCapture { device, reply } => {
            do_start_capture(pipe, &device).await;
            let _ = reply.send(Ok(()));
        }
        PipelineRequest::StopCapture { reply } => {
            pipe.command("stop_capture", serde_json::json!({})).await.ok();
            let _ = reply.send(Ok(()));
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

async fn reload_pipeline(pipe: &mut Pipeline, json: &str) -> Result<()> {
    tracing::info!("standalone: reloading pipeline");
    pipe.shutdown().await;
    let new_pipe = Pipeline::from_json(json)?;
    *pipe = new_pipe;
    pipe.start().await?;
    tracing::info!("standalone: pipeline reloaded");
    Ok(())
}

async fn do_start_capture(pipe: &mut Pipeline, device: &str) {
    tracing::info!(%device, "standalone: starting capture");
    let args = serde_json::json!({ "device": device });
    if let Err(e) = pipe.command("start_capture", args).await {
        tracing::error!(%e, "standalone: start_capture failed");
    }
}
