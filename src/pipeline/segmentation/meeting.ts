import type { SegmentationEngine, TranscriptSegment, SegmentMetadata } from "../interfaces.js";

export type MeetingConfig = {
  maxBlockDurationMs?: number;   // default 300000 (5 min)
  silenceBlockMs?: number;       // default 30000 (30s)
  autoStopSilenceMs?: number;    // default 60000 (60s)
  agentKeywords?: string[];      // e.g. ["molty", "助手"]
};

export class MeetingSegmentation implements SegmentationEngine {
  private messageCallbacks: Array<(message: string, metadata: SegmentMetadata) => void> = [];
  private keywordCallbacks: Array<(text: string) => void> = [];
  private autoStopCallbacks: Array<() => void> = [];
  private buffer: TranscriptSegment[] = [];
  private readonly keywords: string[];
  private readonly maxBlockDurationMs: number;
  private readonly silenceBlockMs: number;
  private readonly autoStopSilenceMs: number;
  private blockTimer: ReturnType<typeof setTimeout> | null = null;
  private silenceTimer: ReturnType<typeof setTimeout> | null = null;
  private autoStopTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(config?: MeetingConfig) {
    this.keywords = (config?.agentKeywords ?? []).map((k) => k.toLowerCase());
    this.maxBlockDurationMs = config?.maxBlockDurationMs ?? 300_000;
    this.silenceBlockMs = config?.silenceBlockMs ?? 30_000;
    this.autoStopSilenceMs = config?.autoStopSilenceMs ?? 60_000;
  }

  onTranscript(segment: TranscriptSegment): void {
    if (!segment.isFinal) return;

    const text = segment.text.trim();
    if (!text) return;

    this.buffer.push(segment);

    // Start max-block timer on first transcript
    if (this.buffer.length === 1) {
      this.startBlockTimer();
    }

    // Check for keyword addressing
    const lower = text.toLowerCase();
    for (const keyword of this.keywords) {
      if (lower.includes(keyword)) {
        for (const cb of this.keywordCallbacks) {
          cb(text);
        }
        break;
      }
    }
  }

  /** Called when a TopicShift event is received from Rust. */
  onTopicShift(): void {
    this.emit();
  }

  onVAD(speaking: boolean): void {
    if (speaking) {
      // Clear silence timers
      this.clearSilenceTimer();
      this.clearAutoStopTimer();
    } else {
      // Start silence-based block emission timer
      this.silenceTimer = setTimeout(() => {
        this.emit();
      }, this.silenceBlockMs);

      // Start auto-stop timer
      this.autoStopTimer = setTimeout(() => {
        this.emit();
        for (const cb of this.autoStopCallbacks) {
          cb();
        }
      }, this.autoStopSilenceMs);
    }
  }

  onMessage(cb: (message: string, metadata: SegmentMetadata) => void): void {
    this.messageCallbacks.push(cb);
  }

  onKeyword(cb: (text: string) => void): void {
    this.keywordCallbacks.push(cb);
  }

  onAutoStop(cb: () => void): void {
    this.autoStopCallbacks.push(cb);
  }

  flush(): string | null {
    this.clearAllTimers();
    return this.emit();
  }

  getBuffer(): string {
    return this.buffer.map((s) => s.text).join(" ").trim();
  }

  private emit(): string | null {
    this.clearBlockTimer();
    this.clearSilenceTimer();

    if (this.buffer.length === 0) return null;

    const text = this.buffer.map((s) => s.text).join(" ").trim();
    if (!text) {
      this.buffer = [];
      return null;
    }

    const metadata: SegmentMetadata = {
      startTime: this.buffer[0].start,
      endTime: this.buffer[this.buffer.length - 1].end,
      segmentCount: this.buffer.length,
    };

    this.buffer = [];

    for (const cb of this.messageCallbacks) {
      cb(text, metadata);
    }

    return text;
  }

  private startBlockTimer(): void {
    this.clearBlockTimer();
    this.blockTimer = setTimeout(() => {
      this.emit();
    }, this.maxBlockDurationMs);
  }

  private clearBlockTimer(): void {
    if (this.blockTimer) {
      clearTimeout(this.blockTimer);
      this.blockTimer = null;
    }
  }

  private clearSilenceTimer(): void {
    if (this.silenceTimer) {
      clearTimeout(this.silenceTimer);
      this.silenceTimer = null;
    }
  }

  private clearAutoStopTimer(): void {
    if (this.autoStopTimer) {
      clearTimeout(this.autoStopTimer);
      this.autoStopTimer = null;
    }
  }

  private clearAllTimers(): void {
    this.clearBlockTimer();
    this.clearSilenceTimer();
    this.clearAutoStopTimer();
  }
}
