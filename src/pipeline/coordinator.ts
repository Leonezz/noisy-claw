import type { SttConfig } from "../ipc/protocol.js";
import type {
  AudioSource,
  STTProvider,
  SegmentationEngine,
  AudioOutput,
  AudioConfig,
  STTConfig,
  SegmentMetadata,
} from "./interfaces.js";

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

export class PipelineCoordinator {
  private readonly components: PipelineComponents;
  private messageCallbacks: Array<(message: string, metadata: SegmentMetadata) => void> = [];
  private active = false;
  private paused = false;
  private echoSuppressed = false;
  private currentConfig: PipelineConfig | null = null;

  constructor(components: PipelineComponents) {
    this.components = components;
    this.wireComponents();
  }

  private wireComponents(): void {
    const { audioSource, sttProvider, segmentation, audioOutput } = this.components;

    // Interruption detection: VAD during echo suppression triggers barge-in.
    // Rust hybrid VAD gate already requires ~192ms of sustained speech at 0.85
    // threshold before emitting Vad{speaking:true}, so we only add a short
    // confirmation window here to avoid double-gating.
    let interruptTimer: ReturnType<typeof setTimeout> | null = null;
    const INTERRUPT_THRESHOLD_MS = 50;

    // AudioSource VAD -> SegmentationEngine (or interruption during echo suppression)
    audioSource.onVAD((speaking) => {
      if (!this.echoSuppressed) {
        segmentation.onVAD(speaking);
        return;
      }

      // During echo suppression: detect sustained speech as user interruption
      console.log(`[noisy-claw] VAD during echo suppression: speaking=${speaking}`);
      if (speaking) {
        if (!interruptTimer) {
          console.log(`[noisy-claw] barge-in timer started (${INTERRUPT_THRESHOLD_MS}ms)`);
          interruptTimer = setTimeout(() => {
            interruptTimer = null;
            console.log("[noisy-claw] barge-in confirmed — stopping TTS output");
            audioOutput.stop();
            this.echoSuppressed = false;
          }, INTERRUPT_THRESHOLD_MS);
        }
      } else {
        if (interruptTimer) {
          console.log("[noisy-claw] barge-in cancelled — speech stopped");
          clearTimeout(interruptTimer);
          interruptTimer = null;
        }
      }
    });

    // AudioSource audio chunks -> STTProvider (when not suppressed)
    audioSource.onAudio((chunk) => {
      if (!this.echoSuppressed) {
        sttProvider.feed(chunk);
      }
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

    // AudioOutput done -> un-suppress STT
    audioOutput.onDone(() => {
      console.log("[noisy-claw] audioOutput.onDone: echoSuppressed=false");
      this.echoSuppressed = false;
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

  async speak(text: string): Promise<void> {
    console.log("[noisy-claw] pipeline.speak: echoSuppressed=true");
    this.echoSuppressed = true;
    await this.components.audioOutput.speak(text);
  }

  speakStart(): void {
    console.log("[noisy-claw] pipeline.speakStart: echoSuppressed=true");
    this.echoSuppressed = true;
    this.components.audioOutput.speakStart?.();
  }

  speakChunk(text: string): void {
    this.components.audioOutput.speakChunk?.(text);
  }

  async speakEnd(): Promise<void> {
    await this.components.audioOutput.speakEnd?.();
    this.echoSuppressed = false;
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
