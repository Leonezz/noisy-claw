/// Shared audio utility functions used by capture, output, and AEC modules.

/// Mix interleaved multi-channel audio to mono by averaging channels.
pub fn mix_to_mono(data: &[f32], channels: usize) -> Vec<f32> {
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
pub fn resample_linear(src: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
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

/// Convert raw PCM bytes (i16 little-endian) to f32 samples in [-1.0, 1.0].
pub fn pcm_bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    let sample_count = bytes.len() / 2;
    let mut out = Vec::with_capacity(sample_count);
    for i in 0..sample_count {
        let lo = bytes[i * 2] as i16;
        let hi = (bytes[i * 2 + 1] as i16) << 8;
        let sample_i16 = lo | hi;
        out.push(sample_i16 as f32 / 32768.0);
    }
    out
}

/// Convert f32 samples to i16 values (for AEC which works with i16).
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| {
            let clamped = s.clamp(-1.0, 1.0);
            (clamped * 32767.0) as i16
        })
        .collect()
}

/// Convert i16 samples back to f32.
pub fn i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples
        .iter()
        .map(|&s| s as f32 / 32768.0)
        .collect()
}

// ---------------------------------------------------------------------------
// Stateful FIR resampler with anti-aliasing filter
// ---------------------------------------------------------------------------

/// Stateful streaming resampler with windowed-sinc FIR low-pass filter.
///
/// For downsampling (e.g. 48k→16k) this applies a low-pass filter before
/// decimation to prevent aliasing.  For upsampling, falls back to
/// `resample_linear` (acceptable — no energy above Nyquist in the source).
pub struct Resampler {
    from_rate: u32,
    to_rate: u32,
    ratio: usize,            // integer decimation ratio (from / to)
    coeffs: Vec<f32>,        // FIR low-pass kernel (symmetric)
    state: Vec<f32>,         // overlap buffer for streaming continuity
    is_integer_decimate: bool,
}

impl Resampler {
    /// Create a new resampler.
    ///
    /// When `from_rate` is an exact integer multiple of `to_rate` (e.g. 48000/16000 = 3),
    /// a polyphase FIR decimator is used. Otherwise falls back to linear interpolation.
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        if from_rate > to_rate && from_rate % to_rate == 0 {
            let ratio = (from_rate / to_rate) as usize;
            let coeffs = Self::design_lowpass(from_rate, to_rate, ratio);
            let state = vec![0.0f32; coeffs.len() - 1];
            Self {
                from_rate,
                to_rate,
                ratio,
                coeffs,
                state,
                is_integer_decimate: true,
            }
        } else {
            Self {
                from_rate,
                to_rate,
                ratio: 1,
                coeffs: Vec::new(),
                state: Vec::new(),
                is_integer_decimate: false,
            }
        }
    }

    /// Process a chunk of input samples, returning resampled output.
    /// Maintains internal state for streaming continuity across calls.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() || self.from_rate == self.to_rate {
            return input.to_vec();
        }
        if !self.is_integer_decimate {
            return resample_linear(input, self.from_rate, self.to_rate);
        }

        // Prepend overlap state from previous chunk
        let overlap = self.state.len(); // filter_len - 1
        let mut buf = Vec::with_capacity(overlap + input.len());
        buf.extend_from_slice(&self.state);
        buf.extend_from_slice(input);

        // FIR filter + decimate
        let out_capacity = input.len() / self.ratio + 1;
        let mut out = Vec::with_capacity(out_capacity);

        // We produce one output sample per `ratio` input samples.
        // The filter is centered at each decimation point.
        let mut i = 0usize;
        while i * self.ratio + overlap < buf.len() {
            let center = i * self.ratio + overlap;
            // Apply FIR: sum coeffs[k] * buf[center - half + k]
            let half = overlap / 2; // (filter_len - 1) / 2
            let start = center.saturating_sub(half);
            let mut acc = 0.0f32;
            for (k, &c) in self.coeffs.iter().enumerate() {
                let idx = start + k;
                if idx < buf.len() {
                    acc += c * buf[idx];
                }
            }
            out.push(acc);
            i += 1;
        }

        // Save overlap state for next call
        let new_state_start = buf.len().saturating_sub(overlap);
        self.state.clear();
        self.state.extend_from_slice(&buf[new_state_start..]);

        out
    }

    /// Reset internal filter state (call on stream discontinuity).
    pub fn reset(&mut self) {
        self.state.fill(0.0);
    }

    /// Design a windowed-sinc low-pass FIR filter for decimation.
    ///
    /// Cutoff is set to `0.95 * (to_rate / 2)` to leave a small transition band
    /// while preserving the passband.  Uses a Kaiser window for good stopband
    /// attenuation (~60 dB).
    fn design_lowpass(from_rate: u32, to_rate: u32, ratio: usize) -> Vec<f32> {
        // Filter length: longer = steeper rolloff.  ~16 * ratio gives ~60dB.
        let num_taps = 16 * ratio + 1; // always odd for symmetry
        let half = (num_taps - 1) as f64 / 2.0;

        // Normalized cutoff frequency (0..1 where 1 = Nyquist of from_rate)
        let fc = (to_rate as f64 * 0.95) / (from_rate as f64); // e.g. 0.317 for 48k→16k

        // Kaiser window parameter (beta ≈ 5.65 gives ~60dB stopband attenuation)
        let beta = 5.65;

        let mut coeffs = Vec::with_capacity(num_taps);
        let mut sum = 0.0f64;

        for n in 0..num_taps {
            let x = n as f64 - half;

            // Sinc
            let sinc = if x.abs() < 1e-10 {
                2.0 * fc
            } else {
                (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
            };

            // Kaiser window: I0(beta * sqrt(1 - (2n/N-1)^2)) / I0(beta)
            let t = 2.0 * n as f64 / (num_taps - 1) as f64 - 1.0;
            let arg = beta * (1.0 - t * t).max(0.0).sqrt();
            let window = bessel_i0(arg) / bessel_i0(beta);

            let val = sinc * window;
            coeffs.push(val as f32);
            sum += val;
        }

        // Normalize for unity DC gain
        let inv_sum = 1.0 / sum as f32;
        for c in &mut coeffs {
            *c *= inv_sum;
        }

        coeffs
    }
}

/// Zeroth-order modified Bessel function of the first kind.
/// Used for Kaiser window computation.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..25 {
        term *= (x / (2.0 * k as f64)) * (x / (2.0 * k as f64));
        sum += term;
        if term < 1e-12 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- mix_to_mono ---

    #[test]
    fn mix_mono_passthrough() {
        let data = vec![0.5, -0.5, 0.25];
        let result = mix_to_mono(&data, 1);
        assert_eq!(result, data);
    }

    #[test]
    fn mix_stereo_to_mono() {
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
        let src: Vec<f32> = (0..48).map(|i| i as f32).collect();
        let result = resample_linear(&src, 48000, 16000);
        assert_eq!(result.len(), 16);
    }

    #[test]
    fn resample_preserves_dc() {
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
        assert!((result[0] - 0.0).abs() < 1e-6);
        assert!((result[result.len() - 1] - 45.0).abs() < 1e-6);
    }

    #[test]
    fn resample_16k_to_48k_upsample() {
        let src: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let result = resample_linear(&src, 16000, 48000);
        assert_eq!(result.len(), 48);
    }

    // --- pcm_bytes_to_f32 ---

    #[test]
    fn pcm_bytes_silence() {
        let bytes = vec![0u8; 4]; // two i16 zero samples
        let result = pcm_bytes_to_f32(&bytes);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], 0.0);
        assert_eq!(result[1], 0.0);
    }

    #[test]
    fn pcm_bytes_max_positive() {
        // i16 max = 32767 = 0xFF 0x7F in little-endian
        let bytes = vec![0xFF, 0x7F];
        let result = pcm_bytes_to_f32(&bytes);
        assert!((result[0] - 1.0).abs() < 0.001);
    }

    #[test]
    fn pcm_bytes_empty() {
        let result = pcm_bytes_to_f32(&[]);
        assert!(result.is_empty());
    }

    // --- f32_to_i16 / i16_to_f32 round-trip ---

    #[test]
    fn f32_i16_round_trip() {
        let original = vec![0.0_f32, 0.5, -0.5, 1.0, -1.0];
        let as_i16 = f32_to_i16(&original);
        let back = i16_to_f32(&as_i16);
        for (a, b) in original.iter().zip(back.iter()) {
            assert!((a - b).abs() < 0.001, "expected {} got {}", a, b);
        }
    }

    #[test]
    fn f32_to_i16_clamps() {
        let over = vec![2.0_f32, -2.0];
        let result = f32_to_i16(&over);
        assert_eq!(result[0], 32767);
        assert_eq!(result[1], -32767);
    }

    // --- Resampler (FIR anti-aliasing) ---

    #[test]
    fn resampler_dc_preservation() {
        let mut r = Resampler::new(48000, 16000);
        let src = vec![0.75f32; 4800]; // 100ms at 48kHz
        let out = r.process(&src);
        assert_eq!(out.len(), 1600); // 100ms at 16kHz
        // Skip first ~20 samples (FIR ramp-up from zero-padded state)
        for (i, &s) in out.iter().enumerate().skip(20) {
            assert!(
                (s - 0.75).abs() < 0.01,
                "DC preservation failed at {}: expected ~0.75 got {}",
                i,
                s
            );
        }
    }

    #[test]
    fn resampler_correct_length_ratio() {
        let mut r = Resampler::new(48000, 16000);
        let src = vec![0.0f32; 48000]; // 1 second
        let out = r.process(&src);
        assert_eq!(out.len(), 16000);
    }

    #[test]
    fn resampler_empty_input() {
        let mut r = Resampler::new(48000, 16000);
        let out = r.process(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn resampler_same_rate_passthrough() {
        let mut r = Resampler::new(16000, 16000);
        let src = vec![0.5f32; 160];
        let out = r.process(&src);
        assert_eq!(out, src);
    }

    #[test]
    fn resampler_streaming_continuity() {
        // Processing chunk-by-chunk should produce same result as all-at-once
        let src = vec![0.75f32; 4800];

        let mut r1 = Resampler::new(48000, 16000);
        let all_at_once = r1.process(&src);

        let mut r2 = Resampler::new(48000, 16000);
        let mut chunked = Vec::new();
        for chunk in src.chunks(480) {
            // 10ms chunks
            chunked.extend(r2.process(chunk));
        }

        assert_eq!(all_at_once.len(), chunked.len());
        for (i, (&a, &b)) in all_at_once.iter().zip(chunked.iter()).enumerate() {
            assert!(
                (a - b).abs() < 0.02,
                "Streaming mismatch at {}: {} vs {}",
                i,
                a,
                b
            );
        }
    }

    #[test]
    fn resampler_aliasing_attenuation() {
        // Generate a 20kHz sine at 48kHz — this should be attenuated to near-zero
        // after downsampling to 16kHz (Nyquist = 8kHz)
        let mut r = Resampler::new(48000, 16000);
        let n = 48000; // 1 second
        let freq = 20000.0f64;
        let src: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / 48000.0).sin() as f32)
            .collect();

        let out = r.process(&src);

        // RMS of output should be very small (< -40dB of input)
        let rms: f32 = (out.iter().map(|&s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(
            rms < 0.01,
            "Aliasing attenuation failed: 20kHz sine RMS after 48k→16k = {} (should be < 0.01)",
            rms
        );
    }

    #[test]
    fn resampler_passband_preservation() {
        // A 1kHz sine should pass through with near-unity gain
        let mut r = Resampler::new(48000, 16000);
        let n = 48000;
        let freq = 1000.0f64;
        let src: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / 48000.0).sin() as f32)
            .collect();

        let out = r.process(&src);

        let in_rms: f32 = (src.iter().map(|&s| s * s).sum::<f32>() / src.len() as f32).sqrt();
        let out_rms: f32 = (out.iter().map(|&s| s * s).sum::<f32>() / out.len() as f32).sqrt();

        let gain = out_rms / in_rms;
        assert!(
            (gain - 1.0).abs() < 0.05,
            "Passband gain for 1kHz: {} (should be ~1.0)",
            gain
        );
    }

    #[test]
    fn resampler_reset_clears_state() {
        let mut r = Resampler::new(48000, 16000);
        let src = vec![1.0f32; 480];
        let _ = r.process(&src);
        r.reset();
        // After reset, state should be zeroed
        assert!(r.state.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn resampler_non_integer_ratio_falls_back() {
        // 44100 → 16000 is not an integer ratio, should fall back to linear
        let mut r = Resampler::new(44100, 16000);
        assert!(!r.is_integer_decimate);
        let src = vec![0.5f32; 441];
        let out = r.process(&src);
        // Should produce ~160 samples (441 * 16000/44100 ≈ 160)
        assert!(out.len() >= 159 && out.len() <= 161);
    }
}
