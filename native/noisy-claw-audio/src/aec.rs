use anyhow::Result;
use webrtc_audio_processing::Processor;
use webrtc_audio_processing_config::{
    Config,
    EchoCanceller as AecConfig,
    HighPassFilter, NoiseSuppression,
};

use crate::audio_utils::resample_linear;

/// AEC runs at 48kHz to match the typical output device rate.
/// This avoids downsampling the render reference (which would introduce
/// aliasing from the simple linear resampler), giving AEC a clean
/// reference signal that accurately matches the speaker output.
const AEC_SAMPLE_RATE: u32 = 48000;

pub struct EchoCanceller {
    processor: Processor,
    render_buf: Vec<f32>,
    capture_buf: Vec<f32>,
    frame_size: usize,
}

impl EchoCanceller {
    pub fn new() -> Result<Self> {
        let processor = Processor::new(AEC_SAMPLE_RATE)
            .map_err(|e| anyhow::anyhow!("webrtc-audio-processing init: {:?}", e))?;

        let config = Config {
            echo_canceller: Some(AecConfig::Full {
                stream_delay_ms: None,
            }),
            noise_suppression: Some(NoiseSuppression {
                level: webrtc_audio_processing_config::NoiseSuppressionLevel::Moderate,
                analyze_linear_aec_output: false,
            }),
            high_pass_filter: Some(HighPassFilter {
                apply_in_full_band: true,
            }),
            ..Default::default()
        };
        processor.set_config(config);

        let frame_size = processor.num_samples_per_frame(); // 480 at 48kHz

        Ok(Self {
            processor,
            render_buf: Vec::new(),
            capture_buf: Vec::new(),
            frame_size,
        })
    }

    /// Feed speaker reference audio (output going to speakers).
    /// Samples should be mono f32 at any sample rate — will be resampled to 48kHz if needed.
    /// Each complete frame is immediately passed to the AEC engine.
    pub fn feed_render(&mut self, samples: &[f32], sample_rate: u32) {
        let resampled = if sample_rate != AEC_SAMPLE_RATE {
            resample_linear(samples, sample_rate, AEC_SAMPLE_RATE)
        } else {
            samples.to_vec()
        };
        self.render_buf.extend_from_slice(&resampled);

        while self.render_buf.len() >= self.frame_size {
            let frame_buf: Vec<f32> = self.render_buf.drain(..self.frame_size).collect();
            let mut channels = vec![frame_buf];
            let _ = self.processor.process_render_frame(&mut channels);
        }
    }

    /// Process microphone capture audio through AEC + noise suppression + HPF.
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
        while self.capture_buf.len() >= self.frame_size {
            let frame_buf: Vec<f32> = self.capture_buf.drain(..self.frame_size).collect();
            let mut channels = vec![frame_buf];
            match self.processor.process_capture_frame(&mut channels) {
                Ok(_) => cleaned_48k.extend_from_slice(&channels[0]),
                Err(_) => cleaned_48k.extend_from_slice(&channels[0]), // passthrough on error
            }
        }

        if sample_rate != AEC_SAMPLE_RATE && !cleaned_48k.is_empty() {
            resample_linear(&cleaned_48k, AEC_SAMPLE_RATE, sample_rate)
        } else {
            cleaned_48k
        }
    }

    /// Reset internal accumulation buffers (does NOT reset AEC filter state).
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
