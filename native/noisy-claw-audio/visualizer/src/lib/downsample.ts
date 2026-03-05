/**
 * Compute min/max envelope from a ring buffer.
 * Returns an array of [max, min] pairs, one per bucket.
 */
export function envelope(
  buffer: Float32Array,
  writeOffset: number,
  numBuckets: number,
): [number, number][] {
  const total = buffer.length
  const samplesPerBucket = total / numBuckets
  const result: [number, number][] = []

  for (let b = 0; b < numBuckets; b++) {
    const start = Math.floor(b * samplesPerBucket)
    const end = Math.floor((b + 1) * samplesPerBucket)
    let min = 0
    let max = 0
    for (let i = start; i < end; i++) {
      const idx = (writeOffset + i) % total
      const s = buffer[idx]
      if (s < min) min = s
      if (s > max) max = s
    }
    result.push([max, min])
  }

  return result
}
