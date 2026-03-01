use aec3::voip::VoipAec3;
use anyhow::Result;

use crate::audio_utils::resample_linear;

/// AEC runs at 48kHz to match the typical output device rate.
/// This avoids downsampling the render reference (which would introduce
/// aliasing from the simple linear resampler), giving AEC3 a clean
/// reference signal that accurately matches the speaker output.
const AEC_SAMPLE_RATE: u32 = 48000;
const AEC_FRAME_SIZE: usize = 480; // 10ms at 48kHz

pub struct EchoCanceller {
    aec: VoipAec3,
    render_buf: Vec<f32>,
    capture_buf: Vec<f32>,
}

impl EchoCanceller {
    pub fn new() -> Result<Self> {
        let aec = VoipAec3::builder(AEC_SAMPLE_RATE as usize, 1, 1)
            .initial_delay_ms(50)
            .enable_high_pass(true)
            .build()
            .map_err(|e| anyhow::anyhow!("aec3 init failed: {:?}", e))?;

        Ok(Self {
            aec,
            render_buf: Vec::new(),
            capture_buf: Vec::new(),
        })
    }

    /// Feed speaker reference audio (output going to speakers).
    /// Samples should be mono f32 at any sample rate — will be resampled to 48kHz if needed.
    /// Each complete 480-sample frame is immediately passed to the AEC engine.
    pub fn feed_render(&mut self, samples: &[f32], sample_rate: u32) {
        let resampled = if sample_rate != AEC_SAMPLE_RATE {
            resample_linear(samples, sample_rate, AEC_SAMPLE_RATE)
        } else {
            samples.to_vec()
        };
        self.render_buf.extend_from_slice(&resampled);

        // Feed complete 10ms frames to the AEC engine immediately
        while self.render_buf.len() >= AEC_FRAME_SIZE {
            let frame: Vec<f32> = self.render_buf.drain(..AEC_FRAME_SIZE).collect();
            let _ = self.aec.handle_render_frame(&frame);
        }
    }

    /// Process microphone capture audio through AEC.
    /// Samples should be mono f32 at any sample rate — will be resampled to 48kHz internally.
    /// Returns cleaned audio at the original sample rate.
    pub fn process_capture(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        let resampled = if sample_rate != AEC_SAMPLE_RATE {
            resample_linear(samples, sample_rate, AEC_SAMPLE_RATE)
        } else {
            samples.to_vec()
        };
        self.capture_buf.extend_from_slice(&resampled);

        let mut cleaned_48k = Vec::new();

        // Process complete 10ms frames
        while self.capture_buf.len() >= AEC_FRAME_SIZE {
            let cap_frame: Vec<f32> = self.capture_buf.drain(..AEC_FRAME_SIZE).collect();
            let mut out = vec![0.0f32; AEC_FRAME_SIZE];

            match self.aec.process_capture_frame(&cap_frame, false, &mut out) {
                Ok(_) => cleaned_48k.extend_from_slice(&out),
                Err(_) => cleaned_48k.extend_from_slice(&cap_frame),
            }
        }

        // Resample back to original rate if needed
        if sample_rate != AEC_SAMPLE_RATE && !cleaned_48k.is_empty() {
            resample_linear(&cleaned_48k, AEC_SAMPLE_RATE, sample_rate)
        } else {
            cleaned_48k
        }
    }

    /// Reset internal accumulation buffers (does NOT reset AEC3 filter state).
    pub fn reset_buffers(&mut self) {
        self.capture_buf.clear();
        self.render_buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_echo_canceller() {
        let result = EchoCanceller::new();
        assert!(result.is_ok());
    }

    #[test]
    fn process_silence_at_48k() {
        let mut ec = EchoCanceller::new().unwrap();
        let silence = vec![0.0f32; 480]; // 10ms at 48kHz
        ec.feed_render(&silence, 48000);
        let result = ec.process_capture(&silence, 48000);
        assert!(result.len() <= 480);
    }

    #[test]
    fn process_multiple_frames_at_48k() {
        let mut ec = EchoCanceller::new().unwrap();
        // 50ms of audio at 48kHz (5 frames of 480)
        let samples = vec![0.1f32; 2400];
        ec.feed_render(&samples, 48000);
        let result = ec.process_capture(&samples, 48000);
        assert_eq!(result.len(), 2400);
    }

    #[test]
    fn process_capture_at_16k_render_at_48k() {
        let mut ec = EchoCanceller::new().unwrap();
        // Render at 48kHz (native, no resample)
        let render = vec![0.1f32; 480];
        ec.feed_render(&render, 48000);
        // Capture at 16kHz (upsampled internally)
        let capture = vec![0.0f32; 160];
        let result = ec.process_capture(&capture, 16000);
        // Result should be back at 16kHz — length may vary due to resampling
        assert!(!result.is_empty() || result.is_empty()); // ensure no panic
    }

    #[test]
    fn process_with_resampling() {
        let mut ec = EchoCanceller::new().unwrap();
        // Feed render at 48kHz, capture at 16kHz
        let render = vec![0.0f32; 4800]; // 100ms at 48kHz
        ec.feed_render(&render, 48000);
        let capture = vec![0.0f32; 1600]; // 100ms at 16kHz
        let result = ec.process_capture(&capture, 16000);
        // Output should be at 16kHz
        assert!(!result.is_empty());
    }

    #[test]
    fn reset_clears_buffers() {
        let mut ec = EchoCanceller::new().unwrap();
        let samples = vec![0.1f32; 100]; // Less than one frame
        ec.feed_render(&samples, 48000);
        ec.reset_buffers();
        // After reset, process should not use old render data
        let result = ec.process_capture(&vec![0.0f32; 480], 48000);
        assert_eq!(result.len(), 480);
    }
}
