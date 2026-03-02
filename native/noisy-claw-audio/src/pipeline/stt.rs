use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::cloud;
use crate::cloud::traits::{RecognizerConfig, SpeechRecognizer};
use crate::protocol::{Event, SttConfig};
use crate::stt::WhisperSTT;

use super::{AudioFrame, VadEvent};

pub enum Control {
    StartCloud(SttConfig, u32),
    StartLocal { model_path: std::path::PathBuf, language: String },
    Stop,
    Shutdown,
}

pub struct Handle {
    pub control_tx: mpsc::Sender<Control>,
    join: JoinHandle<()>,
}

impl Handle {
    pub async fn start_cloud(&self, config: SttConfig, sample_rate: u32) {
        let _ = self
            .control_tx
            .send(Control::StartCloud(config, sample_rate))
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
///
/// Observes:
///   - `is_speaking_tts`: gates audio feeding during TTS playback
pub fn spawn(
    mut audio_rx: mpsc::UnboundedReceiver<AudioFrame>,
    mut vad_rx: mpsc::Receiver<VadEvent>,
    event_tx: mpsc::Sender<Event>,
    is_speaking_tts: watch::Receiver<bool>,
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

        loop {
            tokio::select! {
                Some(ctl) = ctl_rx.recv() => {
                    match ctl {
                        Control::StartCloud(stt_config, sample_rate) => {
                            let provider = stt_config.provider.as_str();
                            let model = stt_config.model.as_deref()
                                .unwrap_or("paraformer-realtime-v2");
                            let api_key = match &stt_config.api_key {
                                Some(k) => k.clone(),
                                None => {
                                    let _ = event_tx.send(Event::Error {
                                        message: "cloud STT requires api_key".to_string(),
                                    }).await;
                                    continue;
                                }
                            };

                            match cloud::create_recognizer(provider, model) {
                                Ok(mut recognizer) => {
                                    let config = RecognizerConfig {
                                        api_key,
                                        endpoint: stt_config.endpoint.clone(),
                                        model: model.to_string(),
                                        languages: stt_config.languages.clone()
                                            .unwrap_or_else(|| vec!["en".to_string()]),
                                        sample_rate,
                                        extra: stt_config.extra.clone()
                                            .unwrap_or_default(),
                                    };
                                    if let Err(e) = recognizer.start(&config).await {
                                        let _ = event_tx.send(Event::Error {
                                            message: format!("cloud STT start failed: {e}"),
                                        }).await;
                                        continue;
                                    }
                                    cloud_recognizer = Some(recognizer);
                                    using_cloud = true;
                                    capture_start_time = Some(Instant::now());
                                    tracing::info!(provider, model, "STT node: cloud started");
                                }
                                Err(e) => {
                                    let _ = event_tx.send(Event::Error {
                                        message: format!("cloud STT init failed: {e}"),
                                    }).await;
                                }
                            }
                        }

                        Control::StartLocal { model_path, language } => {
                            match WhisperSTT::new(&model_path, &language) {
                                Ok(w) => {
                                    whisper_engine = Some(Arc::new(w));
                                    using_cloud = false;
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
                            using_cloud = false;
                            was_speaking = false;
                            speech_buffer.clear();
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

                // Process audio frames
                Some(frame) = audio_rx.recv() => {
                    let speaking_tts = *is_speaking_tts.borrow();

                    // Cloud STT: gate during TTS unless barge-in confirmed
                    if using_cloud && (!speaking_tts || was_speaking) {
                        if let Some(ref mut recognizer) = cloud_recognizer {
                            if let Err(e) = recognizer.feed_audio(&frame.samples).await {
                                tracing::error!(%e, "STT node: cloud feed_audio failed");
                            }
                        }
                    }

                    // Local Whisper: accumulate during speech
                    if !using_cloud && was_speaking {
                        speech_buffer.extend_from_slice(&frame.samples);
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
