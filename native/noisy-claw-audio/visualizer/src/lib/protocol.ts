/** Parsed binary audio frame from the tap server. */
export interface AudioFrame {
  tap: string
  sampleRate: number
  sampleCount: number
  timestamp: number
  samples: Float32Array
}

/** Parsed VAD metadata from the tap server (text frame). */
export interface VadMeta {
  type: 'vad_meta'
  data: string
  timestamp: number
}

/** Dump directory listing. */
export interface DumpEntry {
  name: string
  meta?: {
    created?: string
    taps?: Record<string, { sample_rate: number }>
  }
}

/**
 * Parse a binary WebSocket message into an AudioFrame.
 *
 * Wire format:
 *   [1B tap_name_len][NB tap_name][4B sample_rate LE][4B sample_count LE][8B timestamp f64 LE][N*4B f32 LE samples]
 */
export function parseAudioFrame(data: ArrayBuffer): AudioFrame {
  const view = new DataView(data)
  let offset = 0

  const tapLen = view.getUint8(offset)
  offset += 1

  const tapBytes = new Uint8Array(data, offset, tapLen)
  const tap = new TextDecoder().decode(tapBytes)
  offset += tapLen

  const sampleRate = view.getUint32(offset, true)
  offset += 4

  const sampleCount = view.getUint32(offset, true)
  offset += 4

  const timestamp = view.getFloat64(offset, true)
  offset += 8

  // Copy samples — can't use Float32Array view directly because offset
  // may not be 4-byte aligned (variable-length tap name in header).
  const sampleBytes = new Uint8Array(data, offset, sampleCount * 4)
  const samples = new Float32Array(sampleBytes.buffer.slice(sampleBytes.byteOffset, sampleBytes.byteOffset + sampleBytes.byteLength))

  return { tap, sampleRate, sampleCount, timestamp, samples }
}

/** Compute RMS level of audio samples. */
export function computeRms(samples: Float32Array): number {
  if (samples.length === 0) return 0
  let sum = 0
  for (let i = 0; i < samples.length; i++) {
    sum += samples[i] * samples[i]
  }
  return Math.sqrt(sum / samples.length)
}

/** Convert RMS to dB (relative to full scale). */
export function rmsToDb(rms: number): number {
  if (rms <= 0) return -100
  return 20 * Math.log10(rms)
}

// ── Pipeline introspection types ──────────────────────────────────

export interface PortDescriptor {
  name: string
  port_type: 'Audio' | 'VadEvent' | 'OutputMsg' | 'IpcEvent' | 'Signal'
  direction: 'In' | 'Out'
}

export interface NodeDefinition {
  name: string
  type: string
  properties: Record<string, unknown>
}

export interface LinkDefinition {
  from: string
  to: string
}

export interface PipelineDefinition {
  name: string
  nodes: NodeDefinition[]
  links: LinkDefinition[]
  modes: Record<string, Record<string, unknown>>
}

export interface NodeSnapshot {
  node_type: string
  status: string
  properties: Record<string, unknown>
  metrics: Record<string, unknown>
}

export interface PipelineSnapshot {
  name: string
  current_mode: string | null
  nodes: Record<string, NodeSnapshot>
}

export interface PipelineData {
  definition: PipelineDefinition | null
  snapshot: PipelineSnapshot | null
}
