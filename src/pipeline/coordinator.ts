import type {
  AudioSource,
  STTProvider,
  SegmentationEngine,
  AudioOutput,
  AudioConfig,
  STTConfig,
  SegmentMetadata,
} from "./interfaces.js";
import type { SttConfig } from "../ipc/protocol.js";

export type PipelineConfig = {
  audio: AudioConfig;
  stt: STTConfig;
  sttConfig?: SttConfig;
};

export type PipelineComponents = {
  audioSource: AudioSource;
  sttProvider: STTProvider;
  segmentation: SegmentationEngine;
  audioOutput: AudioOutput;
};

let requestCounter = 0;
function nextRequestId(): string {
  return `req-ts-${String(++requestCounter).padStart(6, "0")}`;
}

// Sentence boundary characters for chunking LLM deltas
const SENTENCE_BOUNDARY = /[。！？.!?\n]/;

export class PipelineCoordinator {
  private readonly components: PipelineComponents;
  private messageCallbacks: Array<(message: string, metadata: SegmentMetadata) => void> = [];
  private active = false;
  private paused = false;
  private currentConfig: PipelineConfig | null = null;
  private activeRequestId: string | null = null;
  // Sentence chunking buffer for streaming TTS
  private chunkBuffer = "";

  constructor(components: PipelineComponents) {
    this.components = components;
    this.wireComponents();
  }

  private wireComponents(): void {
    const { audioSource, sttProvider, segmentation, audioOutput } = this.components;

    // AudioSource VAD -> SegmentationEngine
    // Barge-in is now handled entirely in Rust (VAD node detects speech
    // during TTS and sends barge-in signal to orchestrator).
    audioSource.onVAD((speaking) => {
      segmentation.onVAD(speaking);
    });

    // AudioSource audio chunks -> STTProvider
    // Echo cancellation and VAD gating are handled in Rust, so we always feed.
    audioSource.onAudio((chunk) => {
      sttProvider.feed(chunk);
    });

    // STTProvider transcripts -> SegmentationEngine
    sttProvider.onTranscript((segment) => {
      segmentation.onTranscript(segment);
    });

    // SegmentationEngine messages -> callbacks
    segmentation.onMessage((message, metadata) => {
      for (const cb of this.messageCallbacks) {
        cb(message, metadata);
      }
    });

    // AudioOutput done -> reset active request
    audioOutput.onDone((requestId, reason) => {
      console.log(`[noisy-claw] audioOutput.onDone: requestId=${requestId} reason=${reason}`);
      this.activeRequestId = null;
    });
  }

  start(config: PipelineConfig): void {
    if (this.active) return;
    this.active = true;
    this.paused = false;
    this.currentConfig = config;
    // Pass cloud STT config to audio source if available
    console.log(`[noisy-claw] pipeline.start: sttConfig present=${!!config.sttConfig}, provider=${config.sttConfig?.provider}`);
    const source = this.components.audioSource as { setSttConfig?: (c: unknown) => void };
    if (typeof source.setSttConfig === "function") {
      source.setSttConfig(config.sttConfig);
    }
    this.components.audioSource.start(config.audio);
    this.components.sttProvider.start(config.stt);
  }

  stop(): void {
    if (!this.active) return;
    this.active = false;
    this.paused = false;
    this.components.audioSource.stop();
    this.components.sttProvider.stop();
    this.components.segmentation.flush();
  }

  pause(): void {
    if (!this.active || this.paused) return;
    this.paused = true;
    this.components.audioSource.stop();
    this.components.segmentation.flush();
  }

  resume(): void {
    if (!this.active || !this.paused || !this.currentConfig) return;
    this.paused = false;
    this.components.audioSource.start(this.currentConfig.audio);
  }

  get isPaused(): boolean {
    return this.paused;
  }

  speak(text: string): void {
    const requestId = nextRequestId();
    this.activeRequestId = requestId;
    console.log(`[noisy-claw] pipeline.speak: requestId=${requestId}`);
    this.components.audioOutput.speak(text, requestId);
  }

  speakStart(): void {
    const requestId = nextRequestId();
    this.activeRequestId = requestId;
    this.chunkBuffer = "";
    console.log(`[noisy-claw] pipeline.speakStart: requestId=${requestId}`);
    this.components.audioOutput.speakStart(requestId);
  }

  speakChunk(text: string): void {
    if (!this.activeRequestId) return;
    // Buffer text and flush at sentence boundaries
    this.chunkBuffer += text;
    let boundary = this.chunkBuffer.search(SENTENCE_BOUNDARY);
    while (boundary !== -1) {
      const sentence = this.chunkBuffer.slice(0, boundary + 1);
      this.chunkBuffer = this.chunkBuffer.slice(boundary + 1);
      this.components.audioOutput.speakChunk(sentence, this.activeRequestId);
      boundary = this.chunkBuffer.search(SENTENCE_BOUNDARY);
    }
  }

  speakEnd(): void {
    if (!this.activeRequestId) return;
    // Flush remaining buffered text
    if (this.chunkBuffer.length > 0) {
      this.components.audioOutput.speakChunk(this.chunkBuffer, this.activeRequestId);
      this.chunkBuffer = "";
    }
    this.components.audioOutput.speakEnd(this.activeRequestId);
  }

  onMessage(cb: (message: string, metadata: SegmentMetadata) => void): void {
    this.messageCallbacks.push(cb);
  }

  get isActive(): boolean {
    return this.active;
  }

  get isSpeaking(): boolean {
    return this.components.audioOutput.isPlaying();
  }
}
