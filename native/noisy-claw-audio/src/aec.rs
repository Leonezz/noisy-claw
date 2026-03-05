use anyhow::Result;
use webrtc_audio_processing::Processor;
use webrtc_audio_processing_config::{
    Config,
    EchoCanceller as AecConfig,
    HighPassFilter, NoiseSuppression,
};

use crate::protocol::PIPELINE_SAMPLE_RATE;

pub struct EchoCanceller {
    processor: Processor,
    render_buf: Vec<f32>,
    capture_buf: Vec<f32>,
    frame_size: usize,
}

impl EchoCanceller {
    pub fn new() -> Result<Self> {
        let processor = Processor::new(PIPELINE_SAMPLE_RATE)
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
    /// Samples must be mono f32 at PIPELINE_SAMPLE_RATE (48kHz).
    pub fn feed_render(&mut self, samples: &[f32], sample_rate: u32) {
        debug_assert_eq!(
            sample_rate, PIPELINE_SAMPLE_RATE,
            "AEC feed_render expects {}Hz, got {}Hz",
            PIPELINE_SAMPLE_RATE, sample_rate
        );
        self.render_buf.extend_from_slice(samples);

        while self.render_buf.len() >= self.frame_size {
            let frame_buf: Vec<f32> = self.render_buf.drain(..self.frame_size).collect();
            let mut channels = vec![frame_buf];
            let _ = self.processor.process_render_frame(&mut channels);
        }
    }

    /// Process microphone capture audio through AEC + noise suppression + HPF.
    /// Samples must be mono f32 at PIPELINE_SAMPLE_RATE (48kHz).
    /// Returns cleaned audio at 48kHz.
    pub fn process_capture(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        debug_assert_eq!(
            sample_rate, PIPELINE_SAMPLE_RATE,
            "AEC process_capture expects {}Hz, got {}Hz",
            PIPELINE_SAMPLE_RATE, sample_rate
        );
        self.capture_buf.extend_from_slice(samples);

        let mut cleaned = Vec::new();
        while self.capture_buf.len() >= self.frame_size {
            let frame_buf: Vec<f32> = self.capture_buf.drain(..self.frame_size).collect();
            let mut channels = vec![frame_buf];
            match self.processor.process_capture_frame(&mut channels) {
                Ok(_) => cleaned.extend_from_slice(&channels[0]),
                Err(_) => cleaned.extend_from_slice(&channels[0]),
            }
        }

        cleaned
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
    fn process_100ms_at_48k() {
        let mut ec = EchoCanceller::new().unwrap();
        let render = vec![0.0f32; 4800]; // 100ms at 48kHz
        ec.feed_render(&render, 48000);
        let capture = vec![0.0f32; 4800]; // 100ms at 48kHz
        let result = ec.process_capture(&capture, 48000);
        assert_eq!(result.len(), 4800);
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
