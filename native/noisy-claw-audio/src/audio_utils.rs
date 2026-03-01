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
}
