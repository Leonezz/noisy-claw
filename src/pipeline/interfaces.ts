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

// --- Pipeline Interfaces ---

export interface AudioSource {
  start(config: AudioConfig): void;
  stop(): void;
  onAudio(cb: (chunk: AudioChunk) => void): void;
  onVAD(cb: (speaking: boolean) => void): void;
  onTopicShift?(cb: (similarity: number) => void): void;
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
  getBuffer?(): string;
}

export interface AudioOutput {
  speak(text: string, requestId: string): void;
  speakStart(requestId: string): void;
  speakChunk(text: string, requestId: string): void;
  speakEnd(requestId: string): void;
  stop(): void;
  flush(requestId: string): void;
  isPlaying(): boolean;
  onDone(cb: (requestId: string, reason: string) => void): void;
}
