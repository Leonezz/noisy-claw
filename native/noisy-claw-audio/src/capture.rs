use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::audio_utils::{mix_to_mono, resample_linear};

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

    // Audio util tests (mix_to_mono, resample_linear) are now in audio_utils.rs
}
