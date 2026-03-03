use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapRb,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::audio_utils::resample_linear;

pub struct StreamingOutput {
    _stream: Stream,
    producer: ringbuf::HeapProd<f32>,
    playing: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    /// The actual device output sample rate (may differ from requested TTS rate).
    native_rate: u32,
}

impl StreamingOutput {
    /// Create a streaming cpal output.
    /// `desired_rate` is used as a hint; the actual device rate may differ.
    /// Returns (Self, reference_rx) where reference_rx receives copies of
    /// every output frame for AEC reference.
    pub fn new(desired_rate: u32) -> Result<(Self, mpsc::UnboundedReceiver<Vec<f32>>)> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default output device"))?;

        let device_desc = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_default();
        tracing::info!(device = %device_desc, "streaming output device");

        let default_config = device.default_output_config()?;
        let native_rate = {
            let supported = device.supported_output_configs()?;
            let rate_ok = supported.into_iter().any(|range| {
                range.min_sample_rate() <= desired_rate
                    && desired_rate <= range.max_sample_rate()
            });
            if rate_ok {
                desired_rate
            } else {
                let fallback = default_config.sample_rate();
                tracing::info!(
                    native = fallback,
                    desired = desired_rate,
                    "output device does not support desired rate, will resample on write"
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

        // Ring buffer: 30 seconds at the actual device rate.
        // TTS sends audio faster than real-time, so we need headroom.
        let capacity = (native_rate as usize) * 30;
        let rb = HeapRb::<f32>::new(capacity);
        let (producer, mut consumer) = rb.split();

        let playing = Arc::new(AtomicBool::new(true));
        let finished = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let playing_cb = playing.clone();
        let finished_cb = finished.clone();
        let paused_cb = paused.clone();

        let (ref_tx, ref_rx) = mpsc::unbounded_channel::<Vec<f32>>();
        let ch = channels as usize;

        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], _info| {
                let frame_count = data.len() / ch;
                let mut mono_ref = Vec::with_capacity(frame_count);
                let is_paused = paused_cb.load(Ordering::SeqCst);

                for frame_idx in 0..frame_count {
                    // When paused, output silence without consuming the ring buffer
                    let sample = if is_paused {
                        0.0
                    } else {
                        consumer.try_pop().unwrap_or(0.0)
                    };
                    mono_ref.push(sample);
                    for c in 0..ch {
                        data[frame_idx * ch + c] = sample;
                    }
                }

                // Send reference (silence during pause) to AEC
                let _ = ref_tx.send(mono_ref);

                if !is_paused && finished_cb.load(Ordering::SeqCst) && consumer.is_empty() {
                    playing_cb.store(false, Ordering::SeqCst);
                }
            },
            |err| {
                tracing::error!(%err, "output stream error");
            },
            None,
        )?;

        stream.play()?;

        tracing::info!(
            native_rate,
            channels,
            capacity,
            "streaming output started"
        );

        Ok((
            Self {
                _stream: stream,
                producer,
                playing,
                finished,
                paused,
                native_rate,
            },
            ref_rx,
        ))
    }

    /// Push PCM mono f32 samples into the ring buffer.
    /// `source_rate` is the sample rate of the incoming data (e.g. TTS output rate).
    /// If it differs from the device rate, samples are resampled automatically.
    /// Returns the number of samples written to the ring buffer.
    pub fn write_samples(&mut self, samples: &[f32], source_rate: u32) -> usize {
        if samples.is_empty() {
            return 0;
        }
        let resampled;
        let to_write = if source_rate != self.native_rate {
            resampled = resample_linear(samples, source_rate, self.native_rate);
            &resampled
        } else {
            samples
        };
        self.producer.push_slice(to_write)
    }

    /// Signal that no more samples will be written.
    /// Playback continues until the buffer drains.
    pub fn finish(&self) {
        self.finished.store(true, Ordering::SeqCst);
    }

    /// Stop immediately — mark as done. The cpal callback will output silence.
    pub fn stop(&mut self) {
        self._stream.pause().ok();
        self.finished.store(true, Ordering::SeqCst);
        self.playing.store(false, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
    }

    /// Pause playback — cpal callback outputs silence but ring buffer is preserved.
    /// Used during barge-in evaluation (pause-then-evaluate pattern).
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// Resume playback from where it was paused.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::SeqCst)
    }

    /// Returns a clone of the playing flag for external drain polling.
    pub fn playing_flag(&self) -> Arc<AtomicBool> {
        self.playing.clone()
    }

    /// The actual device output sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.native_rate
    }
}
