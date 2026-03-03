use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::protocol::Event;
use crate::vad::VoiceActivityDetector;

use super::{AudioFrame, VadEvent};

/// ~128ms at 32ms/window — number of consecutive VAD-positive frames
/// required before confirming barge-in during TTS playback.
const BARGE_IN_FRAME_COUNT: u32 = 4;

/// Pre-roll buffer size: ~200ms at 16kHz = 3200 samples.
/// Stores recent audio so that speech onset (lost during the barge-in
/// detection window) can be replayed to STT on confirmed barge-in.
const PRE_ROLL_SAMPLES: usize = 3200;

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

        let mut consecutive_speech_frames: u32 = 0;
        let mut was_speaking = false;
        let mut log_counter: u32 = 0;
        let mut prev_speaking_tts = false;

        // AEC warmup: track when first audio arrives to suppress
        // barge-in during the initial convergence period.
        let mut first_audio_time: Option<Instant> = None;

        // Pre-roll buffer: always stores the last ~200ms of AEC-cleaned audio.
        // On confirmed barge-in, this buffer is replayed to STT so the speech
        // onset (captured during the detection window) is not lost.
        let mut pre_roll: VecDeque<f32> = VecDeque::with_capacity(PRE_ROLL_SAMPLES + 512);

        // Comfort blanking: countdown of VAD windows to suppress barge-in
        // after TTS starts, giving AEC time to converge on the new signal.
        let mut blanking_countdown: u32 = 0;

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
                            consecutive_speech_frames = 0;
                            was_speaking = false;
                            blanking_countdown = 0;
                            tracing::info!("VAD node: reset");
                        }
                        Control::CancelBargeIn => {
                            // False alarm recovery: re-enable audio gating
                            consecutive_speech_frames = 0;
                            if was_speaking {
                                was_speaking = false;
                                let _ = event_tx.send(Event::Vad { speaking: false }).await;
                                let _ = vad_event_tx.send(VadEvent { speaking: false }).await;
                            }
                            tracing::info!("VAD node: barge-in cancelled (false alarm)");
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

                    // Always update pre-roll buffer (ring buffer of recent audio)
                    for &s in &frame.samples {
                        if pre_roll.len() >= PRE_ROLL_SAMPLES {
                            pre_roll.pop_front();
                        }
                        pre_roll.push_back(s);
                    }

                    // Audio forwarding to STT:
                    // - Normal mode (no TTS): always forward
                    // - TTS mode: only forward after barge-in confirmed (was_speaking)
                    if !speaking_tts || was_speaking {
                        let _ = audio_passthrough_tx.send(frame.clone());
                    }

                    // Run VAD inference
                    let Some(ref mut v) = vad else { continue };
                    let results = match v.process(&frame.samples) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!(%e, "VAD node: processing failed");
                            continue;
                        }
                    };

                    for w in results {
                        if speaking_tts {
                            log_counter += 1;

                            if log_counter % 15 == 0 {
                                tracing::info!(
                                    prob = format!("{:.3}", w.speech_prob),
                                    is_speech = w.is_speech,
                                    consecutive = consecutive_speech_frames,
                                    threshold = v.threshold(),
                                    blanking = blanking_countdown,
                                    "VAD node: TTS gate"
                                );
                            }

                            // Comfort blanking: suppress barge-in detection
                            // while AEC converges on new TTS audio
                            if blanking_countdown > 0 {
                                blanking_countdown -= 1;
                                consecutive_speech_frames = 0;
                                continue;
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
                                        pre_roll_samples = pre_roll.len(),
                                        "VAD node: barge-in confirmed, sending pre-roll"
                                    );

                                    // Replay pre-roll buffer to STT so speech onset
                                    // captured during the detection window is preserved
                                    if !pre_roll.is_empty() {
                                        let pre_roll_samples: Vec<f32> =
                                            pre_roll.drain(..).collect();
                                        let _ = audio_passthrough_tx.send(AudioFrame {
                                            samples: pre_roll_samples,
                                            sample_rate: frame.sample_rate,
                                        });
                                    }

                                    let _ = barge_in_tx.send(()).await;
                                    let _ = event_tx.send(Event::Vad { speaking: true }).await;
                                    let _ = vad_event_tx
                                        .send(VadEvent { speaking: true })
                                        .await;
                                    was_speaking = true;
                                }
                            } else {
                                if consecutive_speech_frames > 0 {
                                    tracing::info!(
                                        consecutive_speech_frames,
                                        prob = format!("{:.3}", w.speech_prob),
                                        "VAD node: speech streak broken during TTS"
                                    );
                                }
                                consecutive_speech_frames = 0;
                                if was_speaking {
                                    tracing::info!("VAD node: speech ended during TTS");
                                    let _ =
                                        event_tx.send(Event::Vad { speaking: false }).await;
                                    let _ = vad_event_tx
                                        .send(VadEvent { speaking: false })
                                        .await;
                                    was_speaking = false;
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
