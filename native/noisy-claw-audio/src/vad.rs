use anyhow::Result;
use ndarray::{Array2, Array3};
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;

const WINDOW_SIZE: usize = 512; // 32ms at 16kHz
const CONTEXT_SIZE: usize = 64; // 4ms context prepended to each window
const INPUT_SIZE: usize = CONTEXT_SIZE + WINDOW_SIZE; // 576 total
const SAMPLE_RATE: i64 = 16000;

/// Result of processing one 32ms VAD window.
pub struct VadWindowResult {
    /// Raw speech probability from the model (0.0 – 1.0).
    pub speech_prob: f32,
    /// Whether the probability crossed the current threshold.
    pub is_speech: bool,
    /// State transition: Some(true) = speech started, Some(false) = ended, None = no change.
    pub transition: Option<bool>,
}

pub struct VoiceActivityDetector {
    session: Session,
    // Silero VAD v5 uses a single state tensor instead of separate h/c
    state: Array3<f32>,
    threshold: f32,
    triggered: bool,
    // Rolling context: last 64 samples from previous window
    context: Vec<f32>,
    // Buffer for accumulating samples until we have a full window
    buffer: Vec<f32>,
}

impl VoiceActivityDetector {
    pub fn new(model_path: &Path, threshold: f32) -> Result<Self> {
        let session = Session::builder()?
            .with_intra_threads(1)?
            .commit_from_file(model_path)?;

        Ok(Self {
            session,
            state: Array3::zeros((2, 1, 128)),
            threshold,
            triggered: false,
            context: vec![0.0; CONTEXT_SIZE],
            buffer: Vec::with_capacity(WINDOW_SIZE),
        })
    }

    /// Run inference on a single window, prepending context.
    /// Returns the speech probability and updates internal state + context.
    fn infer(&mut self, window: &[f32]) -> Result<f32> {
        // Prepend context to window: [context(64) | window(512)] = 576 samples
        let mut input_data = Vec::with_capacity(INPUT_SIZE);
        input_data.extend_from_slice(&self.context);
        input_data.extend_from_slice(window);

        let input = Array2::from_shape_vec((1, INPUT_SIZE), input_data)?;
        let sr = ndarray::arr0(SAMPLE_RATE);

        let input_tensor = TensorRef::from_array_view(&input)?;
        let sr_tensor = TensorRef::from_array_view(&sr)?;
        let state_tensor = TensorRef::from_array_view(&self.state)?;

        let outputs = self.session.run(ort::inputs![
            "input" => input_tensor,
            "state" => state_tensor,
            "sr" => sr_tensor,
        ])?;

        let (_shape, prob_data) = outputs["output"].try_extract_tensor::<f32>()?;
        let speech_prob = prob_data[0];

        // Update state from model output
        let (_shape, state_data) = outputs["stateN"].try_extract_tensor::<f32>()?;
        self.state = Array3::from_shape_vec((2, 1, 128), state_data.to_vec())?;

        // Save last CONTEXT_SIZE samples of the full input as context for next call
        // The full input is [context | window], so last 64 samples come from the window
        self.context.clear();
        self.context
            .extend_from_slice(&window[window.len() - CONTEXT_SIZE..]);

        Ok(speech_prob)
    }

    /// Result of processing one VAD window.
    /// Always contains the speech probability; `transition` is set only on state changes.

    /// Process audio samples and return per-window VAD results.
    /// Each processed 32ms window produces a VadWindowResult with the raw
    /// speech probability and an optional state transition.
    pub fn process(&mut self, samples: &[f32]) -> Result<Vec<VadWindowResult>> {
        let mut results = Vec::new();
        self.buffer.extend_from_slice(samples);

        while self.buffer.len() >= WINDOW_SIZE {
            let window: Vec<f32> = self.buffer.drain(..WINDOW_SIZE).collect();
            let speech_prob = self.infer(&window)?;

            let is_speech = speech_prob >= self.threshold;
            let transition = if is_speech != self.triggered {
                self.triggered = is_speech;
                Some(is_speech)
            } else {
                None
            };

            results.push(VadWindowResult {
                speech_prob,
                is_speech,
                transition,
            });
        }

        Ok(results)
    }

    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold;
    }

    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    pub fn is_speaking(&self) -> bool {
        self.triggered
    }

    /// Run inference on a single window and return the raw speech probability.
    /// Used for testing/diagnostics — does NOT update triggered state.
    pub fn probability(&mut self, samples: &[f32]) -> Result<f32> {
        assert!(
            samples.len() == WINDOW_SIZE,
            "expected exactly {WINDOW_SIZE} samples"
        );
        self.infer(samples)
    }

    pub fn reset(&mut self) {
        self.state.fill(0.0);
        self.triggered = false;
        self.context = vec![0.0; CONTEXT_SIZE];
        self.buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn models_dir() -> std::path::PathBuf {
        if let Ok(dir) = std::env::var("NOISY_CLAW_MODELS_DIR") {
            return std::path::PathBuf::from(dir);
        }
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("models")
    }

    fn make_silence() -> Vec<f32> {
        vec![0.0; WINDOW_SIZE]
    }

    /// Generate a voiced-speech-like signal using a pulse train filtered
    /// through a formant resonator, similar to human vowel production.
    fn make_voiced_speech() -> Vec<f32> {
        let sample_rate = SAMPLE_RATE as f32;
        let fundamental = 120.0_f32; // Male pitch
        let pulse_period = (sample_rate / fundamental) as usize;

        // Pulse train excitation
        let mut excitation = vec![0.0_f32; WINDOW_SIZE];
        for i in (0..WINDOW_SIZE).step_by(pulse_period) {
            excitation[i] = 1.0;
        }

        // Simple 2nd-order IIR for ~700 Hz formant
        let fc = 700.0_f32;
        let bw = 100.0_f32;
        let r = (-PI * bw / sample_rate).exp();
        let theta = 2.0 * PI * fc / sample_rate;
        let a1 = -2.0 * r * theta.cos();
        let a2 = r * r;

        let mut out = vec![0.0_f32; WINDOW_SIZE];
        for i in 2..WINDOW_SIZE {
            out[i] = excitation[i] - a1 * out[i - 1] - a2 * out[i - 2];
        }

        // Normalize to speech-like amplitude
        let max_val = out.iter().fold(0.0_f32, |a, &b| a.max(b.abs()));
        if max_val > 0.0 {
            for s in &mut out {
                *s = *s / max_val * 0.4;
            }
        }
        out
    }

    #[test]
    fn vad_silence_returns_low_probability() {
        let model_path = models_dir().join("silero_vad.onnx");
        if !model_path.exists() {
            eprintln!(
                "skipping VAD test: model not found at {}",
                model_path.display()
            );
            return;
        }
        let mut vad = VoiceActivityDetector::new(&model_path, 0.5).unwrap();
        let silence = make_silence();

        for _ in 0..10 {
            let prob = vad.probability(&silence).unwrap();
            assert!(prob < 0.1, "silence should have low probability, got {prob}");
        }
    }

    #[test]
    fn vad_voiced_speech_triggers() {
        let model_path = models_dir().join("silero_vad.onnx");
        if !model_path.exists() {
            eprintln!(
                "skipping VAD test: model not found at {}",
                model_path.display()
            );
            return;
        }
        let mut vad = VoiceActivityDetector::new(&model_path, 0.5).unwrap();
        let speech = make_voiced_speech();

        let mut max_prob = 0.0_f32;
        for _ in 0..30 {
            let prob = vad.probability(&speech).unwrap();
            max_prob = max_prob.max(prob);
        }
        println!("max voiced speech probability: {max_prob}");
        assert!(
            max_prob > 0.3,
            "voiced speech should produce high probability, got {max_prob}"
        );
    }

    #[test]
    fn vad_process_detects_speech_transition() {
        let model_path = models_dir().join("silero_vad.onnx");
        if !model_path.exists() {
            eprintln!(
                "skipping VAD test: model not found at {}",
                model_path.display()
            );
            return;
        }
        let mut vad = VoiceActivityDetector::new(&model_path, 0.5).unwrap();

        // Feed silence first
        let silence = make_silence();
        for _ in 0..5 {
            let results = vad.process(&silence).unwrap();
            for r in &results {
                assert!(r.transition.is_none(), "silence should not trigger");
            }
        }

        // Feed speech — should eventually transition to speaking
        let speech = make_voiced_speech();
        let mut triggered = false;
        for _ in 0..30 {
            let results = vad.process(&speech).unwrap();
            for r in &results {
                if r.transition == Some(true) {
                    triggered = true;
                    break;
                }
            }
            if triggered {
                break;
            }
        }
        assert!(triggered, "speech should trigger VAD transition");
    }
}
