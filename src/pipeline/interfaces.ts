// --- Shared Types ---

export type AudioConfig = {
  device: string;
  sampleRate: number;
};

export type AudioChunk = {
  data: Buffer; // Raw PCM samples (16-bit signed, mono)
  timestamp: number; // Seconds from stream start
};

export type STTConfig = {
  model: string; // e.g. "base", "small", "medium"
  language: string; // e.g. "en", "auto"
};

export type TranscriptSegment = {
  text: string;
  isFinal: boolean;
  start: number; // Seconds
  end: number; // Seconds
  confidence?: number;
};

export type SegmentMetadata = {
  startTime: number;
  endTime: number;
  segmentCount: number;
};

export type TTSOpts = {
  voice?: string;
  speed?: number;
};

// --- Pipeline Interfaces ---

export interface AudioSource {
  start(config: AudioConfig): void;
  stop(): void;
  onAudio(cb: (chunk: AudioChunk) => void): void;
  onVAD(cb: (speaking: boolean) => void): void;
}

export interface STTProvider {
  start(config: STTConfig): void;
  stop(): void;
  feed(chunk: AudioChunk): void;
  onTranscript(cb: (segment: TranscriptSegment) => void): void;
}

export interface SegmentationEngine {
  onTranscript(segment: TranscriptSegment): void;
  onVAD(speaking: boolean): void;
  onMessage(cb: (message: string, metadata: SegmentMetadata) => void): void;
  flush(): string | null;
}

export interface TTSProvider {
  synthesize(text: string, opts?: TTSOpts): Promise<string>; // returns audio file path
}

export interface AudioOutput {
  play(audioPath: string): Promise<void>;
  stop(): void;
  isPlaying(): boolean;
  onDone(cb: () => void): void;
}
