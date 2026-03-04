import type { SegmentationEngine, TranscriptSegment, SegmentMetadata } from "../interfaces.js";

export type DictationConfig = {
  endPhrases?: string[];
};

const DEFAULT_END_PHRASES = ["end dictation", "结束听写"];

export class DictationSegmentation implements SegmentationEngine {
  private messageCallbacks: Array<(message: string, metadata: SegmentMetadata) => void> = [];
  private buffer: TranscriptSegment[] = [];
  private readonly endPhrases: string[];

  constructor(config?: DictationConfig) {
    this.endPhrases = (config?.endPhrases ?? DEFAULT_END_PHRASES).map((p) => p.toLowerCase());
  }

  onTranscript(segment: TranscriptSegment): void {
    if (!segment.isFinal) return;

    const text = segment.text.trim();
    if (!text) return;

    // Check for end phrase match
    const lower = text.toLowerCase();
    const matchedPhrase = this.endPhrases.find((phrase) => lower.endsWith(phrase));

    if (matchedPhrase) {
      // Strip the end phrase from the final segment
      const strippedText = text.slice(0, text.length - matchedPhrase.length).trim();
      if (strippedText) {
        this.buffer.push({ ...segment, text: strippedText });
      }
      this.emit();
    } else {
      this.buffer.push(segment);
    }
  }

  onVAD(_speaking: boolean): void {
    // Dictation ignores silence for segmentation
  }

  onMessage(cb: (message: string, metadata: SegmentMetadata) => void): void {
    this.messageCallbacks.push(cb);
  }

  flush(): string | null {
    return this.emit();
  }

  getBuffer(): string {
    return this.buffer.map((s) => s.text).join(" ").trim();
  }

  private emit(): string | null {
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
}
