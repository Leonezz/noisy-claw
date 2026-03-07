use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::audio_utils::Resampler;
use crate::protocol::Event;
use crate::vad::{VadWindowResult, VoiceActivityDetector};

use super::{AudioFrame, VadState};
use super::dump;

/// ~128ms at 32ms/window — number of consecutive VAD-positive frames
/// required before confirming barge-in during speaker playback.
const BARGE_IN_FRAME_COUNT: u32 = 4;

/// ~192ms at 32ms/window — number of consecutive VAD-negative frames
/// required before ending speech tracking during playback. Prevents brief
/// probability dips from prematurely cutting off audio to STT.
const SPEECH_END_FRAME_COUNT: u32 = 6;

/// Post-flush settling: suppress barge-in detection for this many VAD
/// windows after a flush, giving speaker hardware time to go silent and
/// AEC time to stabilize (~192ms at 32ms/window).
const POST_FLUSH_BLANKING_FRAMES: u32 = 6;

/// Comfort blanking: suppress barge-in detection for this many VAD windows
/// after speaker starts, giving AEC time to converge (~192ms at 32ms/window).
const COMFORT_BLANKING_FRAMES: u32 = 6;

/// AEC warmup: suppress barge-in detection for the first 3 seconds after
/// the VAD node starts receiving audio, giving AEC time to converge on
/// the initial audio streams (capture + render reference).
const AEC_WARMUP_DURATION: Duration = Duration::from_secs(3);

pub enum Control {
    SetThreshold(f32),
    Reset,
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

    pub async fn shutdown(self) {
        let _ = self.control_tx.send(Control::Shutdown).await;
        let _ = self.join.await;
    }

    /// Whether the VAD model was loaded successfully.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }
}

/// Barge-in threshold: raised automatically when speaker is active
/// to reduce false triggers from AEC residual.
const BARGE_IN_THRESHOLD: f32 = 0.85;

/// Spawn the VAD node.
///
/// Inputs:
///   - `audio_rx`:           cleaned audio from AecNode
///   - `is_speaker_active`:  watch state from OutputNode
///
/// Outputs:
///   - `audio_passthrough_tx`: forwards audio to SttNode
///   - `user_speaking_tx`:     watch state → STT and orchestrator
///   - `event_tx`:             IPC events (Event::Vad) to stdout
pub fn spawn(
    mut audio_rx: mpsc::UnboundedReceiver<AudioFrame>,
    audio_passthrough_tx: mpsc::UnboundedSender<AudioFrame>,
    user_speaking_tx: watch::Sender<bool>,
    event_tx: mpsc::Sender<Event>,
    is_speaker_active: watch::Receiver<bool>,
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
        let mut prev_speaker_active = false;

        // Base threshold set by the user/mode; effective threshold is raised
        // during playback to suppress AEC residual false triggers.
        let mut base_threshold = initial_threshold;

        // AEC warmup: track when first audio arrives to suppress
        // barge-in during the initial convergence period.
        let mut first_audio_time: Option<Instant> = None;

        // Comfort blanking: countdown of VAD windows to suppress barge-in
        // after speaker starts/stops, giving AEC time to converge.
        let mut blanking_countdown: u32 = 0;

        // Temporary buffer for VAD results (avoids double-borrow of vad)
        let mut vad_results_buf: Vec<VadWindowResult> = Vec::new();

        loop {
            tokio::select! {
                Some(ctl) = ctl_rx.recv() => {
                    match ctl {
                        Control::SetThreshold(t) => {
                            base_threshold = t;
                            if let Some(ref mut v) = vad {
                                let effective = if *is_speaker_active.borrow() {
                                    t.max(BARGE_IN_THRESHOLD)
                                } else {
                                    t
                                };
                                v.set_threshold(effective);
                                tracing::info!(base = t, effective, "VAD node: threshold updated");
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

                    let speaker_active = *is_speaker_active.borrow();

                    // Detect speaker START transition → comfort blanking + raise threshold
                    if speaker_active && !prev_speaker_active {
                        blanking_countdown = COMFORT_BLANKING_FRAMES;
                        consecutive_speech_frames = 0;
                        if let Some(ref mut v) = vad {
                            v.set_threshold(base_threshold.max(BARGE_IN_THRESHOLD));
                        }
                        tracing::info!(
                            blanking_frames = COMFORT_BLANKING_FRAMES,
                            "VAD node: speaker started, comfort blanking + threshold raised"
                        );
                    }

                    // Detect speaker STOP transition → post-flush blanking + reset + restore threshold
                    if !speaker_active && prev_speaker_active {
                        blanking_countdown = POST_FLUSH_BLANKING_FRAMES;
                        consecutive_speech_frames = 0;
                        consecutive_silence_frames = 0;
                        if was_speaking {
                            was_speaking = false;
                            let _ = user_speaking_tx.send(false);
                            let _ = event_tx.send(Event::Vad { speaking: false }).await;
                        }
                        if let Some(ref mut v) = vad {
                            v.reset();
                            v.set_threshold(base_threshold);
                        }
                        tracing::info!(
                            blanking_frames = POST_FLUSH_BLANKING_FRAMES,
                            "VAD node: speaker stopped, post-flush blanking + threshold restored"
                        );
                    }
                    prev_speaker_active = speaker_active;

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
                            speaker_active,
                        }),
                    });

                    // Process VAD results for barge-in logic and events
                    let results = std::mem::take(&mut vad_results_buf);

                    for w in results {
                        dump::write_metadata("vad", serde_json::json!({
                            "elapsed_ms": first_audio_time.map(|t| t.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0),
                            "speech_prob": w.speech_prob,
                            "is_speech": w.is_speech,
                            "speaker_active": speaker_active,
                            "blanking": blanking_countdown,
                            "was_speaking": was_speaking,
                        }));

                        // Blanking: suppress VAD events during comfort blanking
                        // (speaker start/stop). Must run in ALL modes because
                        // speaker_active is already false when post-flush blanking fires.
                        if blanking_countdown > 0 {
                            blanking_countdown -= 1;
                            consecutive_speech_frames = 0;
                            continue;
                        }

                        if speaker_active {
                            log_counter += 1;

                            if log_counter % 15 == 0 {
                                tracing::debug!(
                                    prob = format!("{:.3}", w.speech_prob),
                                    is_speech = w.is_speech,
                                    consecutive = consecutive_speech_frames,
                                    threshold = vad.as_ref().map(|v| v.threshold()).unwrap_or(0.0),
                                    "VAD node: speaker gate"
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
                                        "VAD node: speech frame detected during playback"
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

                                    let _ = user_speaking_tx.send(true);
                                    let _ = event_tx.send(Event::Vad { speaking: true }).await;
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
                                            "VAD node: speech ended during playback (debounced)"
                                        );
                                        let _ = user_speaking_tx.send(false);
                                        let _ =
                                            event_tx.send(Event::Vad { speaking: false }).await;
                                        was_speaking = false;
                                        consecutive_silence_frames = 0;
                                    }
                                } else if consecutive_speech_frames > 0 {
                                    tracing::info!(
                                        consecutive_speech_frames,
                                        prob = format!("{:.3}", w.speech_prob),
                                        "VAD node: speech streak broken during playback"
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
                                let _ = user_speaking_tx.send(speaking);
                                let _ = event_tx.send(Event::Vad { speaking }).await;
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
