use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::audio_utils::Resampler;
use crate::protocol::Event;
use crate::vad::{VadWindowResult, VoiceActivityDetector};

use super::{AudioFrame, VadEvent, VadState};
use super::dump;

/// ~128ms at 32ms/window — number of consecutive VAD-positive frames
/// required before confirming barge-in during TTS playback.
const BARGE_IN_FRAME_COUNT: u32 = 4;

/// ~192ms at 32ms/window — number of consecutive VAD-negative frames
/// required before ending speech tracking during TTS. Prevents brief
/// probability dips from prematurely cutting off audio to STT.
const SPEECH_END_FRAME_COUNT: u32 = 6;

/// Post-flush settling: suppress barge-in detection for this many VAD
/// windows after a flush, giving speaker hardware time to go silent and
/// AEC time to stabilize (~192ms at 32ms/window).
const POST_FLUSH_BLANKING_FRAMES: u32 = 6;

/// Comfort blanking: suppress barge-in detection for this many VAD windows
/// after TTS starts, giving AEC time to converge (~192ms at 32ms/window).
const COMFORT_BLANKING_FRAMES: u32 = 6;

/// AEC warmup: suppress barge-in detection for the first 3 seconds after
/// the VAD node starts receiving audio, giving AEC time to converge on
/// the initial audio streams (capture + render reference).
const AEC_WARMUP_DURATION: Duration = Duration::from_secs(3);

pub enum Control {
    SetThreshold(f32),
    Reset,
    /// Cancel a pending barge-in (false alarm). Resets gating state so audio
    /// stops flowing to STT during TTS, and emits speaking:false events.
    CancelBargeIn,
    /// Apply post-flush blanking: suppress barge-in detection briefly while
    /// residual speaker audio dies out and AEC stabilizes.
    PostFlushBlanking,
    Shutdown,
}

pub struct Handle {
    pub control_tx: mpsc::Sender<Control>,
    initialized: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

impl Handle {
    pub async fn set_threshold(&self, threshold: f32) {
        let _ = self.control_tx.send(Control::SetThreshold(threshold)).await;
    }

    pub async fn reset(&self) {
        let _ = self.control_tx.send(Control::Reset).await;
    }

    pub async fn cancel_barge_in(&self) {
        let _ = self.control_tx.send(Control::CancelBargeIn).await;
    }

    pub async fn post_flush_blanking(&self) {
        let _ = self.control_tx.send(Control::PostFlushBlanking).await;
    }

    pub async fn shutdown(self) {
        let _ = self.control_tx.send(Control::Shutdown).await;
        let _ = self.join.await;
    }

    /// Whether the VAD model was loaded successfully.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }
}

/// Spawn the VAD node.
///
/// Inputs:
///   - `audio_rx`: cleaned audio from AecNode
///
/// Outputs:
///   - `audio_passthrough_tx`: forwards audio to SttNode
///   - `vad_event_tx`:         VAD transitions to SttNode
///   - `event_tx`:             IPC events (Event::Vad) to stdout
///   - `barge_in_tx`:          fires when barge-in is confirmed during TTS
///
/// Observes:
///   - `is_speaking_tts`: pipeline-wide TTS state for hybrid gate
pub fn spawn(
    mut audio_rx: mpsc::UnboundedReceiver<AudioFrame>,
    audio_passthrough_tx: mpsc::UnboundedSender<AudioFrame>,
    vad_event_tx: mpsc::Sender<VadEvent>,
    event_tx: mpsc::Sender<Event>,
    barge_in_tx: mpsc::Sender<()>,
    is_speaking_tts: watch::Receiver<bool>,
    model_path: PathBuf,
    initial_threshold: f32,
) -> Handle {
    let (ctl_tx, mut ctl_rx) = mpsc::channel(16);
    let initialized = Arc::new(AtomicBool::new(false));

    // Load model synchronously so is_initialized() is accurate
    // before any commands arrive.
    let vad = match VoiceActivityDetector::new(&model_path, initial_threshold) {
        Ok(v) => {
            tracing::info!(path = %model_path.display(), "VAD node: model loaded");
            initialized.store(true, Ordering::SeqCst);
            Some(v)
        }
        Err(e) => {
            tracing::warn!(
                %e, path = %model_path.display(),
                "VAD node: init failed, running in passthrough mode"
            );
            None
        }
    };

    let join = tokio::spawn(async move {
        let mut vad = vad;
        tracing::info!("VAD node: task started");

        // Resample 48kHz pipeline audio to 16kHz for Silero VAD inference
        let mut vad_resampler = Resampler::new(48000, 16000);

        let mut consecutive_speech_frames: u32 = 0;
        let mut consecutive_silence_frames: u32 = 0;
        let mut was_speaking = false;
        let mut log_counter: u32 = 0;
        let mut prev_speaking_tts = false;

        // AEC warmup: track when first audio arrives to suppress
        // barge-in during the initial convergence period.
        let mut first_audio_time: Option<Instant> = None;

        // Comfort blanking: countdown of VAD windows to suppress barge-in
        // after TTS starts, giving AEC time to converge on the new signal.
        let mut blanking_countdown: u32 = 0;

        // Temporary buffer for VAD results (avoids double-borrow of vad)
        let mut vad_results_buf: Vec<VadWindowResult> = Vec::new();

        loop {
            tokio::select! {
                Some(ctl) = ctl_rx.recv() => {
                    match ctl {
                        Control::SetThreshold(t) => {
                            if let Some(ref mut v) = vad {
                                v.set_threshold(t);
                                tracing::info!(threshold = t, "VAD node: threshold updated");
                            }
                        }
                        Control::Reset => {
                            if let Some(ref mut v) = vad {
                                v.reset();
                            }
                            vad_resampler.reset();
                            consecutive_speech_frames = 0;
                            consecutive_silence_frames = 0;
                            was_speaking = false;
                            blanking_countdown = 0;
                            tracing::info!("VAD node: reset");
                        }
                        Control::CancelBargeIn => {
                            // False alarm recovery: re-enable audio gating
                            consecutive_speech_frames = 0;
                            consecutive_silence_frames = 0;
                            if was_speaking {
                                was_speaking = false;
                                let _ = event_tx.send(Event::Vad { speaking: false }).await;
                                let _ = vad_event_tx.send(VadEvent { speaking: false }).await;
                            }
                            tracing::info!("VAD node: barge-in cancelled (false alarm)");
                        }
                        Control::PostFlushBlanking => {
                            blanking_countdown = POST_FLUSH_BLANKING_FRAMES;
                            consecutive_speech_frames = 0;
                            tracing::info!(
                                blanking_frames = POST_FLUSH_BLANKING_FRAMES,
                                "VAD node: post-flush blanking active"
                            );
                        }
                        Control::Shutdown => {
                            tracing::info!("VAD node: shutdown");
                            break;
                        }
                    }
                }

                Some(frame) = audio_rx.recv() => {
                    // Track first audio frame for AEC warmup
                    if first_audio_time.is_none() {
                        first_audio_time = Some(Instant::now());
                        tracing::info!("VAD node: first audio frame, AEC warmup started");
                    }

                    let speaking_tts = *is_speaking_tts.borrow();

                    // Detect TTS start transition → begin comfort blanking
                    if speaking_tts && !prev_speaking_tts {
                        blanking_countdown = COMFORT_BLANKING_FRAMES;
                        consecutive_speech_frames = 0;
                        tracing::info!(
                            blanking_frames = COMFORT_BLANKING_FRAMES,
                            "VAD node: TTS started, comfort blanking active"
                        );
                    }
                    prev_speaking_tts = speaking_tts;

                    // Downsample 48kHz→16kHz for VAD inference (Silero expects 16kHz)
                    let vad_samples = vad_resampler.process(&frame.samples);

                    // Run VAD inference on 16kHz audio
                    let (max_speech_prob, any_is_speech) = if let Some(ref mut v) = vad {
                        match v.process(&vad_samples) {
                            Ok(results) => {
                                let mut max_prob: f32 = 0.0;
                                let mut any_speech = false;
                                for w in &results {
                                    max_prob = max_prob.max(w.speech_prob);
                                    any_speech = any_speech || w.is_speech;
                                }
                                // Process barge-in / transition logic below
                                // (results are consumed in the for loop after forwarding)
                                vad_results_buf.clear();
                                vad_results_buf.extend(results);
                                (max_prob, any_speech)
                            }
                            Err(e) => {
                                tracing::error!(%e, "VAD node: processing failed");
                                vad_results_buf.clear();
                                (0.0, false)
                            }
                        }
                    } else {
                        vad_results_buf.clear();
                        (0.0, false)
                    };

                    // Always forward frame with VAD state attached
                    dump::write("vad_pass", &frame.samples, frame.sample_rate);
                    let _ = audio_passthrough_tx.send(AudioFrame {
                        samples: frame.samples,
                        sample_rate: frame.sample_rate,
                        vad: Some(VadState {
                            speech_prob: max_speech_prob,
                            is_speech: any_is_speech,
                            speaking_tts,
                        }),
                    });

                    // Process VAD results for barge-in logic and events
                    let results = std::mem::take(&mut vad_results_buf);

                    for w in results {
                        dump::write_vad_meta(&format!(
                            "{:.1},{:.4},{},{},{},{}\n",
                            first_audio_time.map(|t| t.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0),
                            w.speech_prob,
                            w.is_speech as u8,
                            speaking_tts as u8,
                            blanking_countdown,
                            was_speaking as u8
                        ));

                        // Blanking: suppress VAD events during comfort blanking
                        // (TTS start) or post-flush settling. Must run in ALL
                        // modes because speaking_tts is already false when
                        // post-flush blanking fires.
                        if blanking_countdown > 0 {
                            blanking_countdown -= 1;
                            consecutive_speech_frames = 0;
                            continue;
                        }

                        if speaking_tts {
                            log_counter += 1;

                            if log_counter % 15 == 0 {
                                tracing::debug!(
                                    prob = format!("{:.3}", w.speech_prob),
                                    is_speech = w.is_speech,
                                    consecutive = consecutive_speech_frames,
                                    threshold = vad.as_ref().map(|v| v.threshold()).unwrap_or(0.0),
                                    "VAD node: TTS gate"
                                );
                            }

                            // AEC warmup: suppress barge-in during initial
                            // convergence period after first audio arrives
                            let in_warmup = first_audio_time
                                .map(|t| t.elapsed() < AEC_WARMUP_DURATION)
                                .unwrap_or(true);
                            if in_warmup {
                                consecutive_speech_frames = 0;
                                continue;
                            }

                            // Hybrid gate: require sustained speech for barge-in
                            if w.is_speech {
                                consecutive_speech_frames += 1;
                                consecutive_silence_frames = 0;
                                if consecutive_speech_frames == 1 {
                                    tracing::info!(
                                        prob = format!("{:.3}", w.speech_prob),
                                        "VAD node: speech frame detected during TTS"
                                    );
                                }
                                if consecutive_speech_frames >= BARGE_IN_FRAME_COUNT
                                    && !was_speaking
                                {
                                    tracing::info!(
                                        consecutive_speech_frames,
                                        prob = format!("{:.3}", w.speech_prob),
                                        "VAD node: barge-in confirmed"
                                    );

                                    let _ = barge_in_tx.send(()).await;
                                    let _ = event_tx.send(Event::Vad { speaking: true }).await;
                                    let _ = vad_event_tx
                                        .send(VadEvent { speaking: true })
                                        .await;
                                    was_speaking = true;
                                }
                            } else {
                                consecutive_speech_frames = 0;
                                if was_speaking {
                                    // Debounce: require sustained silence before
                                    // ending speech tracking (avoids brief prob
                                    // dips from cutting off audio to STT)
                                    consecutive_silence_frames += 1;
                                    if consecutive_silence_frames >= SPEECH_END_FRAME_COUNT {
                                        tracing::info!(
                                            consecutive_silence_frames,
                                            prob = format!("{:.3}", w.speech_prob),
                                            "VAD node: speech ended during TTS (debounced)"
                                        );
                                        let _ =
                                            event_tx.send(Event::Vad { speaking: false }).await;
                                        let _ = vad_event_tx
                                            .send(VadEvent { speaking: false })
                                            .await;
                                        was_speaking = false;
                                        consecutive_silence_frames = 0;
                                    }
                                } else if consecutive_speech_frames > 0 {
                                    tracing::info!(
                                        consecutive_speech_frames,
                                        prob = format!("{:.3}", w.speech_prob),
                                        "VAD node: speech streak broken during TTS"
                                    );
                                }
                            }
                        } else {
                            log_counter = 0;
                            // Normal mode: emit on transitions only
                            if let Some(speaking) = w.transition {
                                tracing::info!(
                                    speaking,
                                    prob = format!("{:.3}", w.speech_prob),
                                    "VAD node: transition"
                                );
                                let _ = event_tx.send(Event::Vad { speaking }).await;
                                let _ = vad_event_tx.send(VadEvent { speaking }).await;
                                was_speaking = speaking;
                            }
                        }
                    }
                }
            }
        }
    });

    Handle {
        control_tx: ctl_tx,
        initialized,
        join,
    }
}
