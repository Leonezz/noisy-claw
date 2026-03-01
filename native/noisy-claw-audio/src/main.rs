mod capture;
mod cloud;
mod playback;
mod protocol;
mod stt;
mod vad;

use anyhow::Result;
use cloud::traits::{RecognizerConfig, SpeechRecognizer, SynthesizerConfig};
use protocol::{Command, Event};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "noisy_claw_audio=info".to_string()),
        )
        .init();

    // Channel for events -> stdout writer task
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(256);

    // Spawn stdout writer task
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

    // Mutable state — lives entirely in the main task, no Arc<Mutex<>> needed
    let mut capture = capture::AudioCapture::new();
    let mut playback: Option<playback::AudioPlayback> = None;
    let mut vad_engine: Option<vad::VoiceActivityDetector> = None;
    let mut stt_engine: Option<Arc<stt::WhisperSTT>> = None;
    let mut speech_buffer: Vec<f32> = Vec::new();
    let mut speech_start_time: Option<Instant> = None;
    let mut capture_start_time: Option<Instant> = None;
    let mut suppress_stt = false;
    let mut was_speaking = false;
    let mut is_speaking_tts = false;

    // Cloud STT state
    let mut cloud_recognizer: Option<Box<dyn SpeechRecognizer>> = None;
    let mut using_cloud_stt = false;

    // Audio frame receiver — set when capture starts
    let mut audio_rx: Option<mpsc::UnboundedReceiver<capture::AudioFrame>> = None;

    // Playback completion receiver — set when playback starts
    let mut playback_done_rx: Option<tokio::sync::oneshot::Receiver<()>> = None;

    // Send ready event
    event_tx.send(Event::Ready).await?;

    // Async stdin reader
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    loop {
        tokio::select! {
            // Branch 1: Read commands from stdin
            line = lines.next_line() => {
                let line = match line? {
                    Some(l) => l,
                    None => break, // EOF — stdin closed
                };
                if line.is_empty() {
                    continue;
                }

                match serde_json::from_str::<Command>(&line) {
                    Ok(cmd) => match cmd {
                        Command::StartCapture { device, sample_rate, stt } => {
                            let models_dir = resolve_models_dir();

                            // Init VAD (always needed — for echo suppression + interruption)
                            if vad_engine.is_none() {
                                let vad_path = models_dir.join("silero_vad.onnx");
                                match vad::VoiceActivityDetector::new(&vad_path, 0.5) {
                                    Ok(v) => vad_engine = Some(v),
                                    Err(e) => {
                                        let _ = event_tx.send(Event::Error {
                                            message: format!("VAD init failed: {e}"),
                                        }).await;
                                        continue;
                                    }
                                }
                            }

                            // Determine STT backend
                            let stt_provider = stt.as_ref()
                                .map(|c| c.provider.as_str())
                                .unwrap_or("whisper");

                            if stt_provider == "whisper" {
                                // Local Whisper STT
                                using_cloud_stt = false;
                                if stt_engine.is_none() {
                                    let stt_filename = std::env::var("NOISY_CLAW_STT_MODEL")
                                        .unwrap_or_else(|_| "ggml-base.bin".to_string());
                                    let model_path = models_dir.join(&stt_filename);
                                    match stt::WhisperSTT::new(&model_path, "en") {
                                        Ok(s) => stt_engine = Some(Arc::new(s)),
                                        Err(e) => {
                                            let _ = event_tx.send(Event::Error {
                                                message: format!("STT init failed: {e}"),
                                            }).await;
                                            continue;
                                        }
                                    }
                                }
                            } else {
                                // Cloud STT
                                let stt_cfg = stt.as_ref().unwrap();
                                let model = stt_cfg.model.as_deref().unwrap_or("paraformer-realtime-v2");
                                let api_key = match &stt_cfg.api_key {
                                    Some(k) => k.clone(),
                                    None => {
                                        let _ = event_tx.send(Event::Error {
                                            message: "cloud STT requires api_key".to_string(),
                                        }).await;
                                        continue;
                                    }
                                };

                                match cloud::create_recognizer(stt_provider, model) {
                                    Ok(mut recognizer) => {
                                        let recognizer_config = RecognizerConfig {
                                            api_key,
                                            endpoint: stt_cfg.endpoint.clone(),
                                            model: model.to_string(),
                                            languages: stt_cfg.languages.clone().unwrap_or_else(|| vec!["en".to_string()]),
                                            sample_rate,
                                            extra: stt_cfg.extra.clone().unwrap_or_default(),
                                        };
                                        if let Err(e) = recognizer.start(&recognizer_config).await {
                                            let _ = event_tx.send(Event::Error {
                                                message: format!("cloud STT start failed: {e}"),
                                            }).await;
                                            continue;
                                        }
                                        cloud_recognizer = Some(recognizer);
                                        using_cloud_stt = true;
                                        tracing::info!(provider = stt_provider, model, "cloud STT started");
                                    }
                                    Err(e) => {
                                        let _ = event_tx.send(Event::Error {
                                            message: format!("cloud STT init failed: {e}"),
                                        }).await;
                                        continue;
                                    }
                                }
                            }

                            // Start capture
                            match capture.start(&device, sample_rate) {
                                Ok(rx) => {
                                    audio_rx = Some(rx);
                                    capture_start_time = Some(Instant::now());
                                    tracing::info!("capture started");
                                }
                                Err(e) => {
                                    let _ = event_tx.send(Event::Error {
                                        message: format!("capture start failed: {e}"),
                                    }).await;
                                }
                            }
                        }

                        Command::StopCapture => {
                            capture.stop();
                            audio_rx = None;

                            // Stop cloud recognizer if active
                            if let Some(ref mut recognizer) = cloud_recognizer {
                                if let Err(e) = recognizer.stop().await {
                                    tracing::error!(%e, "cloud STT stop failed");
                                }
                                cloud_recognizer = None;
                                using_cloud_stt = false;
                            }

                            // Flush remaining speech buffer (local Whisper only)
                            if !using_cloud_stt && !speech_buffer.is_empty() {
                                if let Some(ref stt) = stt_engine {
                                    let samples = std::mem::take(&mut speech_buffer);
                                    let stt = stt.clone();
                                    let tx = event_tx.clone();
                                    let base = compute_base_time(speech_start_time.take(), capture_start_time);
                                    tokio::task::spawn_blocking(move || {
                                        transcribe_and_emit(&stt, &samples, base, &tx);
                                    });
                                }
                            }
                            tracing::info!("capture stopped");
                        }

                        Command::Speak { text, tts } => {
                            suppress_stt = true;
                            is_speaking_tts = true;
                            let _ = event_tx.send(Event::SpeakStarted).await;

                            let model = tts.model.as_deref().unwrap_or("cosyvoice-v3-flash");
                            let provider = tts.provider.as_str();
                            let api_key = match &tts.api_key {
                                Some(k) => k.clone(),
                                None => {
                                    let _ = event_tx.send(Event::Error {
                                        message: "TTS requires api_key".to_string(),
                                    }).await;
                                    let _ = event_tx.send(Event::SpeakDone).await;
                                    suppress_stt = false;
                                    is_speaking_tts = false;
                                    continue;
                                }
                            };

                            match cloud::create_synthesizer(provider, model) {
                                Ok(synthesizer) => {
                                    let synth_config = SynthesizerConfig {
                                        api_key,
                                        endpoint: tts.endpoint.clone(),
                                        model: model.to_string(),
                                        voice: tts.voice.clone().unwrap_or_else(|| "longanyang".to_string()),
                                        format: tts.format.clone().unwrap_or_else(|| "wav".to_string()),
                                        sample_rate: tts.sample_rate.unwrap_or(16000),
                                        speed: tts.speed,
                                        extra: tts.extra.clone().unwrap_or_default(),
                                    };

                                    match synthesizer.synthesize(&text, &synth_config).await {
                                        Ok(audio_path) => {
                                            tracing::info!(path = %audio_path.display(), "TTS synthesis complete");

                                            // Init playback lazily
                                            if playback.is_none() {
                                                match playback::AudioPlayback::new() {
                                                    Ok(p) => playback = Some(p),
                                                    Err(e) => {
                                                        suppress_stt = false;
                                                        is_speaking_tts = false;
                                                        let _ = event_tx.send(Event::Error {
                                                            message: format!("playback init failed: {e}"),
                                                        }).await;
                                                        let _ = event_tx.send(Event::SpeakDone).await;
                                                        continue;
                                                    }
                                                }
                                            }

                                            let pb = playback.as_mut().unwrap();
                                            match pb.play(&audio_path) {
                                                Ok(player) => {
                                                    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
                                                    playback_done_rx = Some(done_rx);
                                                    tokio::task::spawn_blocking(move || {
                                                        player.sleep_until_end();
                                                        let _ = done_tx.send(());
                                                    });
                                                }
                                                Err(e) => {
                                                    suppress_stt = false;
                                                    is_speaking_tts = false;
                                                    let _ = event_tx.send(Event::Error {
                                                        message: format!("playback failed: {e}"),
                                                    }).await;
                                                    let _ = event_tx.send(Event::SpeakDone).await;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            suppress_stt = false;
                                            is_speaking_tts = false;
                                            let _ = event_tx.send(Event::Error {
                                                message: format!("TTS synthesis failed: {e}"),
                                            }).await;
                                            let _ = event_tx.send(Event::SpeakDone).await;
                                        }
                                    }
                                }
                                Err(e) => {
                                    suppress_stt = false;
                                    is_speaking_tts = false;
                                    let _ = event_tx.send(Event::Error {
                                        message: format!("TTS init failed: {e}"),
                                    }).await;
                                    let _ = event_tx.send(Event::SpeakDone).await;
                                }
                            }
                        }

                        Command::StopSpeaking => {
                            if let Some(ref mut pb) = playback {
                                pb.stop();
                            }
                            if is_speaking_tts {
                                let _ = event_tx.send(Event::SpeakDone).await;
                            }
                            suppress_stt = false;
                            is_speaking_tts = false;
                            playback_done_rx = None;
                            tracing::info!("speaking stopped");
                        }

                        Command::PlayAudio { path } => {
                            suppress_stt = true;
                            // Init playback lazily (requires audio output device)
                            if playback.is_none() {
                                match playback::AudioPlayback::new() {
                                    Ok(p) => playback = Some(p),
                                    Err(e) => {
                                        suppress_stt = false;
                                        let _ = event_tx.send(Event::Error {
                                            message: format!("playback init failed: {e}"),
                                        }).await;
                                        continue;
                                    }
                                }
                            }
                            let pb = playback.as_mut().unwrap();
                            match pb.play(std::path::Path::new(&path)) {
                                Ok(player) => {
                                    tracing::info!(%path, "playback started");
                                    // Wait for playback completion in a blocking task
                                    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
                                    playback_done_rx = Some(done_rx);
                                    tokio::task::spawn_blocking(move || {
                                        player.sleep_until_end();
                                        let _ = done_tx.send(());
                                    });
                                }
                                Err(e) => {
                                    suppress_stt = false;
                                    let _ = event_tx.send(Event::Error {
                                        message: format!("playback failed: {e}"),
                                    }).await;
                                }
                            }
                        }

                        Command::StopPlayback => {
                            if let Some(ref mut pb) = playback {
                                pb.stop();
                            }
                            suppress_stt = false;
                            playback_done_rx = None;
                            tracing::info!("playback stopped");
                        }

                        Command::GetStatus => {
                            let _ = event_tx.send(Event::Status {
                                capturing: capture.is_running(),
                                playing: playback.as_ref().map_or(false, |p| p.is_playing()),
                                speaking: is_speaking_tts,
                            }).await;
                        }

                        Command::Shutdown => {
                            capture.stop();
                            if let Some(ref mut pb) = playback {
                                pb.stop();
                            }
                            if let Some(ref mut recognizer) = cloud_recognizer {
                                let _ = recognizer.stop().await;
                            }
                            tracing::info!("shutting down");
                            break;
                        }
                    },
                    Err(e) => {
                        tracing::error!(%e, %line, "failed to parse command");
                        let _ = event_tx.send(Event::Error {
                            message: format!("invalid command: {e}"),
                        }).await;
                    }
                }
            }

            // Branch 2: Process audio frames from capture
            Some(frame) = async {
                match audio_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                // Always run VAD (even during playback, for interruption detection)
                if let Some(ref mut vad) = vad_engine {
                    match vad.process(&frame) {
                        Ok(transitions) => {
                            for speaking in transitions {
                                tracing::info!(speaking, "VAD transition");
                                let _ = event_tx.send(Event::Vad { speaking }).await;

                                if speaking && !was_speaking {
                                    speech_start_time = Some(Instant::now());
                                }

                                if !speaking && was_speaking && !suppress_stt {
                                    if using_cloud_stt {
                                        // Cloud STT: results come asynchronously via poll_result
                                        // No action needed on speech end — cloud handles segmentation
                                    } else {
                                        // Local Whisper: offload transcription to blocking task
                                        if let Some(ref stt) = stt_engine {
                                            let samples = std::mem::take(&mut speech_buffer);
                                            let stt = stt.clone();
                                            let tx = event_tx.clone();
                                            let base = compute_base_time(speech_start_time.take(), capture_start_time);
                                            tokio::task::spawn_blocking(move || {
                                                transcribe_and_emit(&stt, &samples, base, &tx);
                                            });
                                        }
                                    }
                                }

                                was_speaking = speaking;
                            }
                        }
                        Err(e) => {
                            tracing::error!(%e, "VAD processing failed");
                        }
                    }
                }

                // Feed audio to cloud STT (if active and not suppressed)
                if using_cloud_stt && !suppress_stt {
                    if let Some(ref mut recognizer) = cloud_recognizer {
                        if let Err(e) = recognizer.feed_audio(&frame).await {
                            tracing::error!(%e, "cloud STT feed_audio failed");
                        }
                    }
                }

                // Accumulate audio for local Whisper STT (only when not suppressed)
                if !using_cloud_stt && !suppress_stt && was_speaking {
                    speech_buffer.extend_from_slice(&frame);
                }
            }

            // Branch 3: Poll cloud STT results
            result = async {
                if using_cloud_stt {
                    if let Some(ref mut recognizer) = cloud_recognizer {
                        return recognizer.poll_result().await;
                    }
                }
                // No cloud STT — pend forever
                std::future::pending::<Result<Option<cloud::traits::RecognitionResult>>>().await
            } => {
                match result {
                    Ok(Some(recognition)) => {
                        let _ = event_tx.send(Event::Transcript {
                            text: recognition.text,
                            is_final: recognition.is_final,
                            start: recognition.start_time,
                            end: recognition.end_time,
                            confidence: recognition.confidence,
                        }).await;
                    }
                    Ok(None) => {
                        // No result yet — yield briefly to avoid busy-spinning
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    Err(e) => {
                        tracing::error!(%e, "cloud STT poll_result failed");
                    }
                }
            }

            // Branch 4: Playback completion
            Ok(()) = async {
                match playback_done_rx.as_mut() {
                    Some(rx) => rx.await.map_err(|_| ()),
                    None => std::future::pending::<std::result::Result<(), ()>>().await,
                }
            } => {
                let was_tts = is_speaking_tts;
                suppress_stt = false;
                is_speaking_tts = false;
                if let Some(ref pb) = playback {
                    pb.set_done();
                }
                playback_done_rx = None;

                if was_tts {
                    let _ = event_tx.send(Event::SpeakDone).await;
                } else {
                    let _ = event_tx.send(Event::PlaybackDone).await;
                }
            }
        }
    }

    Ok(())
}

/// Compute the base timestamp for transcript segments.
fn compute_base_time(speech_start: Option<Instant>, capture_start: Option<Instant>) -> f64 {
    match (speech_start, capture_start) {
        (Some(st), Some(ct)) => st.duration_since(ct).as_secs_f64(),
        _ => 0.0,
    }
}

/// Run STT and send transcript events. Called from spawn_blocking.
fn transcribe_and_emit(
    stt: &stt::WhisperSTT,
    samples: &[f32],
    base_time: f64,
    event_tx: &mpsc::Sender<Event>,
) {
    match stt.transcribe(samples) {
        Ok(segments) => {
            for seg in segments {
                let _ = event_tx.blocking_send(Event::Transcript {
                    text: seg.text,
                    is_final: seg.is_final,
                    start: base_time + seg.start,
                    end: base_time + seg.end,
                    confidence: None,
                });
            }
        }
        Err(e) => {
            tracing::error!(%e, "STT transcription failed");
            let _ = event_tx.blocking_send(Event::Error {
                message: format!("STT failed: {e}"),
            });
        }
    }
}

fn resolve_models_dir() -> PathBuf {
    // Prefer explicit path from parent process
    if let Ok(dir) = std::env::var("NOISY_CLAW_MODELS_DIR") {
        let p = PathBuf::from(&dir);
        if p.exists() {
            return p;
        }
        tracing::warn!(path = %dir, "NOISY_CLAW_MODELS_DIR set but path does not exist, falling back");
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
    use std::time::{Duration, Instant};

    // --- compute_base_time ---

    #[test]
    fn base_time_both_some() {
        let capture_start = Instant::now();
        let speech_start = capture_start + Duration::from_secs(5);
        let base = compute_base_time(Some(speech_start), Some(capture_start));
        // Should be approximately 5.0 seconds
        assert!((base - 5.0).abs() < 0.01);
    }

    #[test]
    fn base_time_speech_none() {
        let capture_start = Instant::now();
        let base = compute_base_time(None, Some(capture_start));
        assert_eq!(base, 0.0);
    }

    #[test]
    fn base_time_capture_none() {
        let speech_start = Instant::now();
        let base = compute_base_time(Some(speech_start), None);
        assert_eq!(base, 0.0);
    }

    #[test]
    fn base_time_both_none() {
        let base = compute_base_time(None, None);
        assert_eq!(base, 0.0);
    }

    // --- resolve_models_dir ---

    #[test]
    fn resolve_models_dir_returns_path() {
        let path = resolve_models_dir();
        // Should return some path (either exe-relative or fallback "models")
        assert!(!path.as_os_str().is_empty());
    }

    #[test]
    fn resolve_models_dir_fallback_is_models() {
        // When no models/ dir exists next to the exe, it falls back to "models"
        let path = resolve_models_dir();
        // The fallback path ends with "models"
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(name, "models");
    }
}
