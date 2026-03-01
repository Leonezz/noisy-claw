import type { SegmentationEngine, TranscriptSegment, SegmentMetadata } from "../interfaces.js";

export type VADSilenceConfig = {
  silenceThresholdMs: number; // default 700ms
};

export class VADSilenceSegmentation implements SegmentationEngine {
  private messageCallbacks: Array<(message: string, metadata: SegmentMetadata) => void> = [];
  private segments: TranscriptSegment[] = [];
  private speaking = false;
  private silenceTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly silenceThresholdMs: number;

  constructor(config?: Partial<VADSilenceConfig>) {
    this.silenceThresholdMs = config?.silenceThresholdMs ?? 700;
  }

  onTranscript(segment: TranscriptSegment): void {
    this.segments.push(segment);
  }

  onVAD(speaking: boolean): void {
    this.speaking = speaking;

    if (speaking) {
      // User started speaking — cancel any pending silence timer
      if (this.silenceTimer) {
        clearTimeout(this.silenceTimer);
        this.silenceTimer = null;
      }
    } else {
      // User stopped speaking — start silence timer
      this.silenceTimer = setTimeout(() => {
        this.emitTurn();
      }, this.silenceThresholdMs);
    }
  }

  onMessage(cb: (message: string, metadata: SegmentMetadata) => void): void {
    this.messageCallbacks.push(cb);
  }

  flush(): string | null {
    if (this.silenceTimer) {
      clearTimeout(this.silenceTimer);
      this.silenceTimer = null;
    }
    return this.emitTurn();
  }

  private emitTurn(): string | null {
    if (this.segments.length === 0) {
      return null;
    }

    const text = this.segments
      .map((s) => s.text)
      .join(" ")
      .trim();
    if (!text) {
      this.segments = [];
      return null;
    }

    const metadata: SegmentMetadata = {
      startTime: this.segments[0].start,
      endTime: this.segments[this.segments.length - 1].end,
      segmentCount: this.segments.length,
    };

    this.segments = [];

    for (const cb of this.messageCallbacks) {
      cb(text, metadata);
    }

    return text;
  }
}
