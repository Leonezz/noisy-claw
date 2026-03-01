use anyhow::Result;
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct TranscriptSegment {
    pub text: String,
    pub start: f64, // seconds
    pub end: f64,   // seconds
    pub is_final: bool,
}

pub struct WhisperSTT {
    ctx: WhisperContext,
    language: String,
}

impl WhisperSTT {
    pub fn new(model_path: &Path, language: &str) -> Result<Self> {
        let ctx = WhisperContext::new_with_params(
            model_path.to_str().unwrap(),
            WhisperContextParameters::default(),
        )?;

        Ok(Self {
            ctx,
            language: language.to_string(),
        })
    }

    /// Transcribe a buffer of audio samples (f32, mono, 16kHz).
    pub fn transcribe(&self, samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        if self.language != "auto" {
            params.set_language(Some(&self.language));
        }

        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_single_segment(false);

        let mut state = self.ctx.create_state()?;
        state.full(params, samples)?;

        let n_segments = state.full_n_segments();
        let mut segments = Vec::with_capacity(n_segments as usize);

        for i in 0..n_segments {
            let Some(seg) = state.get_segment(i) else {
                continue;
            };
            let text = match seg.to_str() {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };
            let start = seg.start_timestamp() as f64 / 100.0; // centiseconds to seconds
            let end = seg.end_timestamp() as f64 / 100.0;

            if !text.is_empty() {
                segments.push(TranscriptSegment {
                    text,
                    start,
                    end,
                    is_final: true,
                });
            }
        }

        Ok(segments)
    }
}
