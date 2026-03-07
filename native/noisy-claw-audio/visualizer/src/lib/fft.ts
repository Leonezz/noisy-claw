/**
 * Radix-2 FFT (in-place, Cooley-Tukey).
 * Input length must be a power of 2.
 * Returns magnitude spectrum (first N/2 bins) in dB, normalized to [0, 1].
 */
export function fftMagnitude(samples: Float32Array, fftSize: number): Float32Array {
  const n = fftSize
  // Copy into real/imag arrays with Hann window
  const real = new Float32Array(n)
  const imag = new Float32Array(n)
  const offset = Math.max(0, samples.length - n)
  for (let i = 0; i < n; i++) {
    const w = 0.5 * (1 - Math.cos((2 * Math.PI * i) / (n - 1)))
    real[i] = (samples[offset + i] ?? 0) * w
  }

  // Bit-reversal permutation
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1
    while (j & bit) {
      j ^= bit
      bit >>= 1
    }
    j ^= bit
    if (i < j) {
      const tr = real[i]; real[i] = real[j]; real[j] = tr
      const ti = imag[i]; imag[i] = imag[j]; imag[j] = ti
    }
  }

  // FFT butterfly
  for (let len = 2; len <= n; len <<= 1) {
    const halfLen = len >> 1
    const angle = (-2 * Math.PI) / len
    const wR = Math.cos(angle)
    const wI = Math.sin(angle)
    for (let i = 0; i < n; i += len) {
      let curR = 1, curI = 0
      for (let j = 0; j < halfLen; j++) {
        const a = i + j
        const b = a + halfLen
        const tR = curR * real[b] - curI * imag[b]
        const tI = curR * imag[b] + curI * real[b]
        real[b] = real[a] - tR
        imag[b] = imag[a] - tI
        real[a] += tR
        imag[a] += tI
        const nextR = curR * wR - curI * wI
        curI = curR * wI + curI * wR
        curR = nextR
      }
    }
  }

  // Magnitude in dB, normalized
  const half = n >> 1
  const mags = new Float32Array(half)
  const minDb = -80
  for (let i = 0; i < half; i++) {
    const mag = Math.sqrt(real[i] * real[i] + imag[i] * imag[i]) / half
    const db = mag > 0 ? 20 * Math.log10(mag) : minDb
    mags[i] = Math.max(0, Math.min(1, (db - minDb) / -minDb))
  }
  return mags
}
