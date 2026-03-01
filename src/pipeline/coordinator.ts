import type { SttConfig } from "../ipc/protocol.js";
import type {
  AudioSource,
  STTProvider,
  SegmentationEngine,
  TTSProvider,
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
  ttsProvider: TTSProvider;
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

    // AudioSource VAD -> SegmentationEngine + echo cancel
    audioSource.onVAD((speaking) => {
      segmentation.onVAD(speaking);

      // Interruption: user speaks during playback
      if (speaking && this.echoSuppressed) {
        audioOutput.stop();
        this.echoSuppressed = false;
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
      this.echoSuppressed = false;
    });
  }

  start(config: PipelineConfig): void {
    if (this.active) return;
    this.active = true;
    this.paused = false;
    this.currentConfig = config;
    // Pass cloud STT config to audio source if available
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
    const audioPath = await this.components.ttsProvider.synthesize(text);
    this.echoSuppressed = true;
    await this.components.audioOutput.play(audioPath);
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
