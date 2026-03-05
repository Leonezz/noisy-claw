use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Sleep;

use crate::audio_utils::Resampler;
use crate::cloud;
use crate::cloud::traits::{RecognizerConfig, SpeechRecognizer};
use crate::protocol::{Event, SttConfig};
use crate::stt::WhisperSTT;

use super::{AudioFrame, VadEvent};

const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(2);

/// Cloud/local STT always receives 16kHz audio (downsampled from pipeline rate).
const STT_SAMPLE_RATE: u32 = 16000;

pub enum Control {
    StartCloud(SttConfig),
    StartLocal { model_path: std::path::PathBuf, language: String },
    Stop,
    Shutdown,
}

pub struct Handle {
    pub control_tx: mpsc::Sender<Control>,
    join: JoinHandle<()>,
}

impl Handle {
    pub async fn start_cloud(&self, config: SttConfig) {
        let _ = self
            .control_tx
            .send(Control::StartCloud(config))
            .await;
    }

    pub async fn start_local(&self, model_path: std::path::PathBuf, language: String) {
        let _ = self
            .control_tx
            .send(Control::StartLocal {
                model_path,
                language,
            })
            .await;
    }

    pub async fn stop(&self) {
        let _ = self.control_tx.send(Control::Stop).await;
    }

    pub async fn shutdown(self) {
        let _ = self.control_tx.send(Control::Shutdown).await;
        let _ = self.join.await;
    }
}

/// Spawn the STT node.
///
/// Inputs:
///   - `audio_rx`:  cleaned audio from VadNode (passthrough)
///   - `vad_rx`:    VAD transitions from VadNode
///
/// Outputs:
///   - `event_tx`:  IPC events (Event::Transcript, Event::Error)
pub fn spawn(
    mut audio_rx: mpsc::UnboundedReceiver<AudioFrame>,
    mut vad_rx: mpsc::Receiver<VadEvent>,
    event_tx: mpsc::Sender<Event>,
) -> Handle {
    let (ctl_tx, mut ctl_rx) = mpsc::channel(16);

    let join = tokio::spawn(async move {
        tracing::info!("STT node: task started");
        let mut cloud_recognizer: Option<Box<dyn SpeechRecognizer>> = None;
        let mut whisper_engine: Option<Arc<WhisperSTT>> = None;
        let mut using_cloud = false;
        let mut was_speaking = false;
        let mut speech_buffer: Vec<f32> = Vec::new();
        let mut speech_start_time: Option<Instant> = None;
        let mut capture_start_time: Option<Instant> = None;

        // Resample 48kHz pipeline audio to 16kHz for STT providers
        let mut stt_resampler = Resampler::new(48000, STT_SAMPLE_RATE);

        // Diagnostics
        let mut audio_frame_count: u64 = 0;
        let mut audio_sample_count: u64 = 0;
        let mut stt_sample_count: u64 = 0;
        let mut last_stats = Instant::now();

        // Retry state for cloud reconnection
        let mut cloud_config: Option<SttConfig> = None;
        let mut retry_delay = INITIAL_RETRY_DELAY;
        let mut retry_timer: Option<std::pin::Pin<Box<Sleep>>> = None;

        loop {
            tokio::select! {
                Some(ctl) = ctl_rx.recv() => {
                    match ctl {
                        Control::StartCloud(stt_config) => {
                            // Store config for reconnection
                            cloud_config = Some(stt_config.clone());
                            retry_delay = INITIAL_RETRY_DELAY;
                            retry_timer = None;
                            stt_resampler.reset();

                            match try_start_cloud(&stt_config).await {
                                Ok(recognizer) => {
                                    cloud_recognizer = Some(recognizer);
                                    using_cloud = true;
                                    capture_start_time = Some(Instant::now());
                                    tracing::info!(
                                        provider = %stt_config.provider,
                                        "STT node: cloud started"
                                    );
                                }
                                Err(e) => {
                                    // Start failed — keep config, schedule retry
                                    using_cloud = true;
                                    tracing::error!(%e, "STT node: cloud start failed, will retry");
                                    let _ = event_tx.send(Event::Error {
                                        message: format!("cloud STT start failed: {e}"),
                                    }).await;
                                    retry_timer = Some(Box::pin(tokio::time::sleep(retry_delay)));
                                }
                            }
                        }

                        Control::StartLocal { model_path, language } => {
                            match WhisperSTT::new(&model_path, &language) {
                                Ok(w) => {
                                    whisper_engine = Some(Arc::new(w));
                                    using_cloud = false;
                                    cloud_config = None;
                                    retry_timer = None;
                                    stt_resampler.reset();
                                    capture_start_time = Some(Instant::now());
                                    tracing::info!("STT node: local Whisper started");
                                }
                                Err(e) => {
                                    let _ = event_tx.send(Event::Error {
                                        message: format!("STT init failed: {e}"),
                                    }).await;
                                }
                            }
                        }

                        Control::Stop => {
                            // Flush remaining speech buffer for local Whisper
                            if !using_cloud && !speech_buffer.is_empty() {
                                if let Some(ref stt) = whisper_engine {
                                    let samples = std::mem::take(&mut speech_buffer);
                                    let stt = stt.clone();
                                    let tx = event_tx.clone();
                                    let base = compute_base_time(
                                        speech_start_time.take(),
                                        capture_start_time,
                                    );
                                    tokio::task::spawn_blocking(move || {
                                        transcribe_and_emit(&stt, &samples, base, &tx);
                                    });
                                }
                            }
                            if let Some(ref mut recognizer) = cloud_recognizer {
                                if let Err(e) = recognizer.stop().await {
                                    tracing::error!(%e, "STT node: cloud stop failed");
                                }
                            }
                            cloud_recognizer = None;
                            cloud_config = None;
                            using_cloud = false;
                            was_speaking = false;
                            speech_buffer.clear();
                            retry_timer = None;
                            tracing::info!("STT node: stopped");
                        }

                        Control::Shutdown => {
                            if let Some(ref mut recognizer) = cloud_recognizer {
                                let _ = recognizer.stop().await;
                            }
                            tracing::info!("STT node: shutdown");
                            break;
                        }
                    }
                }

                // Retry timer fired — attempt reconnection
                _ = async {
                    match retry_timer.as_mut() {
                        Some(timer) => timer.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => {
                    retry_timer = None;
                    if let Some(ref cfg) = cloud_config {
                        tracing::info!(
                            delay_secs = retry_delay.as_secs(),
                            "STT node: attempting cloud reconnection"
                        );
                        match try_start_cloud(cfg).await {
                            Ok(recognizer) => {
                                cloud_recognizer = Some(recognizer);
                                retry_delay = INITIAL_RETRY_DELAY;
                                capture_start_time = Some(Instant::now());
                                tracing::info!("STT node: cloud reconnected");
                            }
                            Err(e) => {
                                // Exponential backoff, capped
                                retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
                                tracing::warn!(
                                    %e,
                                    next_retry_secs = retry_delay.as_secs(),
                                    "STT node: cloud reconnection failed, scheduling retry"
                                );
                                retry_timer = Some(Box::pin(tokio::time::sleep(retry_delay)));
                            }
                        }
                    }
                }

                // Process audio frames (with VAD state attached)
                Some(frame) = audio_rx.recv() => {
                    audio_frame_count += 1;
                    audio_sample_count += frame.samples.len() as u64;

                    // Downsample 48kHz→16kHz for STT providers
                    let stt_samples = stt_resampler.process(&frame.samples);
                    stt_sample_count += stt_samples.len() as u64;

                    // Periodic diagnostics
                    if last_stats.elapsed() >= Duration::from_secs(5) {
                        tracing::debug!(
                            audio_frames = audio_frame_count,
                            audio_samples = audio_sample_count,
                            stt_samples = stt_sample_count,
                            input_sr = frame.sample_rate,
                            last_frame_len = frame.samples.len(),
                            last_stt_len = stt_samples.len(),
                            using_cloud,
                            has_recognizer = cloud_recognizer.is_some(),
                            "STT node: stats"
                        );
                        last_stats = Instant::now();
                    }

                    // Cloud streaming STT: feed all audio continuously.
                    // VAD state is available via frame.vad for future use.
                    if using_cloud {
                        if let Some(ref mut recognizer) = cloud_recognizer {
                            if let Err(e) = recognizer.feed_audio(&stt_samples).await {
                                tracing::error!(%e, "STT node: cloud connection lost");
                                let _ = event_tx.send(Event::Error {
                                    message: format!("cloud STT disconnected: {e}"),
                                }).await;
                                // Drop zombie, schedule retry
                                cloud_recognizer = None;
                                retry_timer = Some(Box::pin(tokio::time::sleep(retry_delay)));
                            }
                        }
                    }

                    // Local Whisper: accumulate 16kHz audio during speech (VAD-gated)
                    if !using_cloud && was_speaking {
                        speech_buffer.extend_from_slice(&stt_samples);
                    }
                }

                // Process VAD events
                Some(vad_event) = vad_rx.recv() => {
                    let prev_speaking = was_speaking;
                    was_speaking = vad_event.speaking;

                    if vad_event.speaking && !prev_speaking {
                        speech_start_time = Some(Instant::now());
                    }

                    if !vad_event.speaking && prev_speaking && !using_cloud {
                        // End of speech — transcribe for local Whisper
                        if let Some(ref stt) = whisper_engine {
                            let samples = std::mem::take(&mut speech_buffer);
                            let stt = stt.clone();
                            let tx = event_tx.clone();
                            let base = compute_base_time(
                                speech_start_time.take(),
                                capture_start_time,
                            );
                            tokio::task::spawn_blocking(move || {
                                transcribe_and_emit(&stt, &samples, base, &tx);
                            });
                        }
                    }
                }

                // Poll cloud STT results
                result = async {
                    if using_cloud {
                        if let Some(ref mut recognizer) = cloud_recognizer {
                            return recognizer.poll_result().await;
                        }
                    }
                    std::future::pending::<Result<Option<cloud::traits::RecognitionResult>>>().await
                } => {
                    match result {
                        Ok(Some(recognition)) => {
                            tracing::info!(
                                text = %recognition.text,
                                is_final = recognition.is_final,
                                "STT node: transcript"
                            );
                            let _ = event_tx.send(Event::Transcript {
                                text: recognition.text,
                                is_final: recognition.is_final,
                                start: recognition.start_time,
                                end: recognition.end_time,
                                confidence: recognition.confidence,
                            }).await;
                        }
                        Ok(None) => {
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                        Err(e) => {
                            tracing::error!(%e, "STT node: cloud poll_result failed");
                            let _ = event_tx.send(Event::Error {
                                message: format!("cloud STT disconnected: {e}"),
                            }).await;
                            cloud_recognizer = None;
                            retry_timer = Some(Box::pin(tokio::time::sleep(retry_delay)));
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

/// Create and start a cloud recognizer from config.
/// Always starts at STT_SAMPLE_RATE (16kHz) — the STT node handles downsampling.
async fn try_start_cloud(
    stt_config: &SttConfig,
) -> Result<Box<dyn SpeechRecognizer>> {
    let provider = stt_config.provider.as_str();
    let model = stt_config
        .model
        .as_deref()
        .unwrap_or("paraformer-realtime-v2");
    let api_key = stt_config
        .api_key
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("cloud STT requires api_key"))?
        .clone();

    let mut recognizer = cloud::create_recognizer(provider, model)?;
    let config = RecognizerConfig {
        api_key,
        endpoint: stt_config.endpoint.clone(),
        model: model.to_string(),
        languages: stt_config
            .languages
            .clone()
            .unwrap_or_else(|| vec!["en".to_string()]),
        sample_rate: STT_SAMPLE_RATE,
        extra: stt_config.extra.clone().unwrap_or_default(),
    };
    recognizer.start(&config).await?;
    Ok(recognizer)
}

fn compute_base_time(speech_start: Option<Instant>, capture_start: Option<Instant>) -> f64 {
    match (speech_start, capture_start) {
        (Some(st), Some(ct)) => st.duration_since(ct).as_secs_f64(),
        _ => 0.0,
    }
}

fn transcribe_and_emit(
    stt: &WhisperSTT,
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
            tracing::error!(%e, "STT node: transcription failed");
            let _ = event_tx.blocking_send(Event::Error {
                message: format!("STT failed: {e}"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn base_time_both_some() {
        let capture_start = Instant::now();
        let speech_start = capture_start + Duration::from_secs(5);
        let base = compute_base_time(Some(speech_start), Some(capture_start));
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

}
