use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct AudioCapture {
    stream: Option<Stream>,
    running: Arc<AtomicBool>,
}

pub type AudioFrame = Vec<f32>; // Mono f32 samples at target sample rate

impl AudioCapture {
    pub fn new() -> Self {
        Self {
            stream: None,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start capturing from the specified input device.
    /// Returns a tokio receiver that yields audio frames resampled to `target_rate` Hz mono.
    /// Uses an unbounded channel because cpal's audio callback is real-time
    /// and must never block.
    pub fn start(
        &mut self,
        device_name: &str,
        target_rate: u32,
    ) -> Result<mpsc::UnboundedReceiver<AudioFrame>> {
        let host = cpal::default_host();

        let device = if device_name == "default" {
            host.default_input_device()
                .ok_or_else(|| anyhow!("no default input device"))?
        } else {
            host.input_devices()?
                .find(|d| {
                    d.description()
                        .map(|desc| desc.name() == device_name)
                        .unwrap_or(false)
                })
                .ok_or_else(|| anyhow!("input device '{}' not found", device_name))?
        };

        let device_desc = device.description().map(|d| d.name().to_string()).unwrap_or_default();
        tracing::info!(device = %device_desc, "using input device");

        // Determine device native rate — use target if supported, else device default
        let default_config = device.default_input_config()?;
        let native_rate = {
            let supported = device.supported_input_configs()?;
            let rate_ok = supported.into_iter().any(|range| {
                range.min_sample_rate() <= target_rate && target_rate <= range.max_sample_rate()
            });
            if rate_ok {
                target_rate
            } else {
                let fallback = default_config.sample_rate();
                tracing::info!(
                    native = fallback,
                    target = target_rate,
                    "device does not support target rate, will resample"
                );
                fallback
            }
        };

        let channels = default_config.channels();
        let config = cpal::StreamConfig {
            channels,
            sample_rate: native_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        tracing::info!(
            native_rate,
            target_rate,
            channels,
            "audio capture configured"
        );

        let (tx, rx) = mpsc::unbounded_channel::<AudioFrame>();
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();

        // Build a resampler that lives inside the audio callback
        let needs_resample = native_rate != target_rate;
        let needs_mix = channels > 1;
        let ch = channels as usize;

        let stream = device.build_input_stream(
            &config,
            move |data: &[f32], _info| {
                if !running.load(Ordering::SeqCst) {
                    return;
                }

                // Step 1: Mix to mono if multi-channel
                let mono: Vec<f32> = if needs_mix {
                    mix_to_mono(data, ch)
                } else {
                    data.to_vec()
                };

                // Step 2: Resample to target rate if needed
                let resampled = if needs_resample {
                    resample_linear(&mono, native_rate, target_rate)
                } else {
                    mono
                };

                let _ = tx.send(resampled);
            },
            |err| {
                tracing::error!(%err, "audio capture error");
            },
            None,
        )?;

        stream.play()?;
        self.stream = Some(stream);

        Ok(rx)
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.stream = None;
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// Mix interleaved multi-channel audio to mono by averaging channels.
fn mix_to_mono(data: &[f32], channels: usize) -> Vec<f32> {
    let frame_count = data.len() / channels;
    let inv = 1.0 / channels as f32;
    let mut mono = Vec::with_capacity(frame_count);
    for i in 0..frame_count {
        let sum: f32 = data[i * channels..(i + 1) * channels].iter().sum();
        mono.push(sum * inv);
    }
    mono
}

/// Linear-interpolation resampler.
/// Converts `src` from `from_rate` Hz to `to_rate` Hz.
fn resample_linear(src: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if src.is_empty() || from_rate == to_rate {
        return src.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((src.len() as f64) / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    let last = (src.len() - 1) as f64;

    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let pos = pos.min(last);
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;

        let sample = if idx + 1 < src.len() {
            src[idx] * (1.0 - frac) + src[idx + 1] * frac
        } else {
            src[idx]
        };
        out.push(sample);
    }

    out
}

/// List available input devices.
pub fn list_input_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let devices: Vec<String> = host
        .input_devices()?
        .filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
        .collect();
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_capture_not_running() {
        let capture = AudioCapture::new();
        assert!(!capture.is_running());
    }

    #[test]
    fn stop_on_idle_is_noop() {
        let mut capture = AudioCapture::new();
        capture.stop(); // should not panic
        assert!(!capture.is_running());
    }

    #[test]
    fn start_with_nonexistent_device_returns_error() {
        let mut capture = AudioCapture::new();
        let result = capture.start("__nonexistent_device_12345__", 16000);
        assert!(result.is_err());
        assert!(!capture.is_running());
    }

    #[test]
    fn list_input_devices_returns_ok() {
        let result = list_input_devices();
        assert!(result.is_ok());
    }

    // --- mix_to_mono ---

    #[test]
    fn mix_mono_passthrough() {
        let data = vec![0.5, -0.5, 0.25];
        let result = mix_to_mono(&data, 1);
        assert_eq!(result, data);
    }

    #[test]
    fn mix_stereo_to_mono() {
        // L=1.0 R=0.0 → 0.5, L=0.0 R=1.0 → 0.5
        let data = vec![1.0, 0.0, 0.0, 1.0];
        let result = mix_to_mono(&data, 2);
        assert_eq!(result, vec![0.5, 0.5]);
    }

    #[test]
    fn mix_stereo_averages_correctly() {
        let data = vec![0.6, 0.4, -0.2, 0.8];
        let result = mix_to_mono(&data, 2);
        assert!((result[0] - 0.5).abs() < 1e-6);
        assert!((result[1] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn mix_empty_input() {
        let result = mix_to_mono(&[], 2);
        assert!(result.is_empty());
    }

    // --- resample_linear ---

    #[test]
    fn resample_same_rate_passthrough() {
        let data = vec![1.0, 2.0, 3.0];
        let result = resample_linear(&data, 48000, 48000);
        assert_eq!(result, data);
    }

    #[test]
    fn resample_empty_input() {
        let result = resample_linear(&[], 48000, 16000);
        assert!(result.is_empty());
    }

    #[test]
    fn resample_48k_to_16k_ratio() {
        // 48 input samples at 48kHz = 1ms → should produce 16 samples at 16kHz
        let src: Vec<f32> = (0..48).map(|i| i as f32).collect();
        let result = resample_linear(&src, 48000, 16000);
        assert_eq!(result.len(), 16);
    }

    #[test]
    fn resample_preserves_dc() {
        // Constant signal should stay constant after resampling
        let src = vec![0.75_f32; 480];
        let result = resample_linear(&src, 48000, 16000);
        for sample in &result {
            assert!((sample - 0.75).abs() < 1e-6, "expected 0.75 got {}", sample);
        }
    }

    #[test]
    fn resample_first_and_last() {
        let src: Vec<f32> = (0..48).map(|i| i as f32).collect();
        let result = resample_linear(&src, 48000, 16000);
        // First output sample should be src[0]
        assert!((result[0] - 0.0).abs() < 1e-6);
        // Last output sample at index 15 maps to source pos 15*3=45
        assert!((result[result.len() - 1] - 45.0).abs() < 1e-6);
    }

    #[test]
    fn resample_16k_to_48k_upsample() {
        // 16 input samples at 16kHz → should produce 48 samples at 48kHz
        let src: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let result = resample_linear(&src, 16000, 48000);
        assert_eq!(result.len(), 48);
    }
}
