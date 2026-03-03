mod aec;
mod audio_utils;
mod capture;
mod cloud;
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
use tokio::sync::{mpsc, watch};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);
fn next_request_id() -> String {
    let n = REQUEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("req-{n:06}")
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

    // ── IPC event channel → stdout writer ──────────────────────────────
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(256);
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event) {
                let stdout = std::io::stdout();
                let mut stdout = stdout.lock();
                let _ = writeln!(stdout, "{}", json);
                let _ = stdout.flush();
            }
        }
    });

    // ── Pipeline-wide state ────────────────────────────────────────────
    let (tts_speaking_tx, tts_speaking_rx) = watch::channel(false);

    // ── Data channels between nodes ────────────────────────────────────
    let (capture_tx, capture_rx) = mpsc::unbounded_channel();     // capture → aec
    let (render_ref_tx, render_ref_rx) = mpsc::unbounded_channel(); // output → aec
    let (cleaned_tx, cleaned_rx) = mpsc::unbounded_channel();     // aec → vad
    let (vad_audio_tx, vad_audio_rx) = mpsc::unbounded_channel(); // vad → stt (passthrough)
    let (vad_event_tx, vad_event_rx) = mpsc::channel(64);        // vad → stt
    let (output_msg_tx, output_msg_rx) = mpsc::channel(64);      // tts → output
    let (internal_tx, mut internal_rx) = mpsc::channel(16);       // output → orchestrator
    let (barge_in_tx, mut barge_in_rx) = mpsc::channel::<()>(4);  // vad → orchestrator (barge-in)

    // ── Spawn pipeline nodes ───────────────────────────────────────────
    let models_dir = resolve_models_dir();

    tracing::info!(models_dir = %models_dir.display(), "orchestrator: spawning pipeline nodes");

    let capture_handle = pipeline::capture::spawn(capture_tx);
    tracing::info!("orchestrator: capture node spawned");

    let aec_handle = pipeline::aec::spawn(capture_rx, render_ref_rx, cleaned_tx);
    tracing::info!("orchestrator: AEC node spawned");

    let vad_handle = pipeline::vad::spawn(
        cleaned_rx,
        vad_audio_tx,
        vad_event_tx,
        event_tx.clone(),
        barge_in_tx,
        tts_speaking_rx.clone(),
        models_dir.join("silero_vad.onnx"),
        0.5,
    );
    tracing::info!(
        vad_initialized = vad_handle.is_initialized(),
        "orchestrator: VAD node spawned"
    );

    let stt_handle = pipeline::stt::spawn(
        vad_audio_rx,
        vad_event_rx,
        event_tx.clone(),
    );
    tracing::info!("orchestrator: STT node spawned");

    let tts_handle = pipeline::tts::spawn(output_msg_tx.clone(), event_tx.clone());
    tracing::info!("orchestrator: TTS node spawned");

    let output_handle = pipeline::output::spawn(output_msg_rx, render_ref_tx, internal_tx);
    tracing::info!("orchestrator: output node spawned — pipeline ready");

    // ── File-based playback (not part of the pipeline) ─────────────────
    let mut playback_engine: Option<playback::AudioPlayback> = None;
    let mut playback_done_rx: Option<tokio::sync::oneshot::Receiver<()>> = None;

    // ── Orchestrator state ─────────────────────────────────────────────
    let mut is_speaking_tts = false;
    let mut active_request_id: Option<String> = None;

    // ── Ready ──────────────────────────────────────────────────────────
    event_tx.send(Event::Ready).await?;

    // ── IPC command loop ───────────────────────────────────────────────
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
                            &capture_handle,
                            &vad_handle,
                            &stt_handle,
                            &tts_handle,
                            &output_handle,
                            &output_msg_tx,
                            &mut playback_engine,
                            &mut playback_done_rx,
                            &tts_speaking_tx,
                            &event_tx,
                            &models_dir,
                            &mut is_speaking_tts,
                            &mut active_request_id,
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

            // ── VAD barge-in: immediate flush cascade ────────────────
            Some(()) = barge_in_rx.recv() => {
                if is_speaking_tts {
                    let req_id = active_request_id.take().unwrap_or_default();
                    tracing::info!(%req_id, "orchestrator: barge-in confirmed, flushing");

                    // Full flush cascade
                    let tts_ack = tts_handle.flush(pipeline::FlushSignal::Flush {
                        request_id: req_id.clone(),
                    }).await;
                    tracing::info!(node = ?tts_ack.node, "orchestrator: TTS flush ack");

                    let out_ack = output_handle.flush(pipeline::FlushSignal::Flush {
                        request_id: req_id.clone(),
                    }).await;
                    tracing::info!(node = ?out_ack.node, "orchestrator: Output flush ack");

                    is_speaking_tts = false;
                    tts_speaking_tx.send_replace(false);

                    // Keep threshold at 0.85 briefly and apply post-flush
                    // blanking so residual speaker audio doesn't re-trigger
                    // barge-in. The blanking countdown will expire, then
                    // normal-mode VAD (threshold 0.5) resumes naturally
                    // when speaking_tts becomes false.
                    vad_handle.post_flush_blanking().await;

                    vad_handle.reset().await;

                    let _ = event_tx.send(Event::SpeakDone {
                        request_id: Some(req_id),
                        reason: "interrupted".to_string(),
                    }).await;
                }
            }

            // ── Output node signals speak done ─────────────────────────
            Some(internal_event) = internal_rx.recv() => {
                match internal_event {
                    pipeline::OutputNodeEvent::SpeakDone => {
                        tracing::info!("orchestrator: speak done (natural)");
                        if is_speaking_tts {
                            let req_id = active_request_id.take();
                            is_speaking_tts = false;
                            tts_speaking_tx.send_replace(false);
                            vad_handle.post_flush_blanking().await;
        
                            vad_handle.reset().await;
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
    capture_handle.shutdown().await;
    aec_handle.shutdown().await;
    vad_handle.shutdown().await;
    stt_handle.shutdown().await;
    tts_handle.shutdown().await;
    output_handle.shutdown().await;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_command(
    cmd: Command,
    capture_handle: &pipeline::capture::Handle,
    vad_handle: &pipeline::vad::Handle,
    stt_handle: &pipeline::stt::Handle,
    tts_handle: &pipeline::tts::Handle,
    output_handle: &pipeline::output::Handle,
    output_msg_tx: &mpsc::Sender<pipeline::OutputMessage>,
    playback_engine: &mut Option<playback::AudioPlayback>,
    playback_done_rx: &mut Option<tokio::sync::oneshot::Receiver<()>>,
    tts_speaking_tx: &watch::Sender<bool>,
    event_tx: &mpsc::Sender<Event>,
    models_dir: &PathBuf,
    is_speaking_tts: &mut bool,
    active_request_id: &mut Option<String>,
) -> bool {
    tracing::info!(?cmd, "orchestrator: command received");

    match cmd {
        Command::StartCapture {
            device,
            sample_rate,
            stt,
        } => {
            // Check VAD initialization
            if !vad_handle.is_initialized() {
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
                %device, sample_rate, stt_provider = %stt_provider_str,
                "orchestrator: starting capture pipeline"
            );

            if stt_provider == "whisper" {
                let stt_filename = std::env::var("NOISY_CLAW_STT_MODEL")
                    .unwrap_or_else(|_| "ggml-base.bin".to_string());
                tracing::info!(%stt_filename, "orchestrator: starting local Whisper STT");
                stt_handle
                    .start_local(models_dir.join(&stt_filename), "en".to_string())
                    .await;
            } else if let Some(stt_config) = stt {
                tracing::info!(stt_provider = %stt_provider_str, "orchestrator: starting cloud STT");
                stt_handle.start_cloud(stt_config, sample_rate).await;
            }

            // Start capture
            capture_handle.start(&device, sample_rate).await;
            tracing::info!(
                %device, sample_rate,
                is_capturing = capture_handle.is_capturing(),
                "orchestrator: capture started"
            );
        }

        Command::StopCapture => {
            capture_handle.stop().await;
            stt_handle.stop().await;
            tracing::info!(
                is_capturing = capture_handle.is_capturing(),
                "orchestrator: capture stopped"
            );
        }

        Command::Speak { text, tts, request_id: cmd_req_id } => {
            let req_id = cmd_req_id.unwrap_or_else(next_request_id);
            *active_request_id = Some(req_id.clone());
            *is_speaking_tts = true;
            tts_speaking_tx.send_replace(true);
            vad_handle.set_threshold(0.85).await;
            let _ = event_tx.send(Event::SpeakStarted {
                request_id: Some(req_id.clone()),
            }).await;

            tts_handle.speak(text, tts, pipeline::RequestId(req_id)).await;
        }

        Command::SpeakStart { tts, request_id: cmd_req_id } => {
            let req_id = cmd_req_id.unwrap_or_else(next_request_id);
            *active_request_id = Some(req_id.clone());
            *is_speaking_tts = true;
            tts_speaking_tx.send_replace(true);
            vad_handle.set_threshold(0.85).await;
            let _ = event_tx.send(Event::SpeakStarted {
                request_id: Some(req_id.clone()),
            }).await;

            tts_handle.speak_start(tts, pipeline::RequestId(req_id)).await;
        }

        Command::SpeakChunk { text } => {
            tts_handle.speak_chunk(text).await;
        }

        Command::SpeakEnd => {
            tts_handle.speak_end().await;
            // Output node will signal SpeakDone when buffer drains
        }

        Command::FlushSpeak { request_id } => {
            if *is_speaking_tts {
                let _ = tts_handle.flush(pipeline::FlushSignal::Flush {
                    request_id: request_id.clone(),
                }).await;
                let _ = output_handle.flush(pipeline::FlushSignal::Flush {
                    request_id: request_id.clone(),
                }).await;
                *is_speaking_tts = false;
                tts_speaking_tx.send_replace(false);
                vad_handle.post_flush_blanking().await;
                vad_handle.reset().await;
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
                // Flush cascade
                let _ = tts_handle.flush(pipeline::FlushSignal::FlushAll).await;
                let _ = output_handle.flush(pipeline::FlushSignal::FlushAll).await;
                // Reset pipeline state
                *is_speaking_tts = false;
                tts_speaking_tx.send_replace(false);
                vad_handle.post_flush_blanking().await;
                vad_handle.reset().await;
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

        Command::GetStatus => {
            let _ = event_tx
                .send(Event::Status {
                    capturing: capture_handle.is_capturing(),
                    playing: playback_engine
                        .as_ref()
                        .map_or(false, |p| p.is_playing()),
                    speaking: *is_speaking_tts,
                })
                .await;
        }

        Command::Shutdown => {
            capture_handle.stop().await;
            stt_handle.stop().await;
            tts_handle.stop().await;
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
}
