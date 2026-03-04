import { describe, it, expect, vi } from "vitest";
import { DictationSegmentation } from "./dictation.js";
import type { TranscriptSegment, SegmentMetadata } from "../interfaces.js";

function makeSegment(text: string, start = 0, end = 1): TranscriptSegment {
  return { text, isFinal: true, start, end };
}

describe("DictationSegmentation", () => {
  it("accumulates transcripts without emitting", () => {
    const seg = new DictationSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("Hello world"));
    seg.onTranscript(makeSegment("How are you"));

    expect(cb).not.toHaveBeenCalled();
  });

  it("emits on end phrase match", () => {
    const seg = new DictationSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("Hello world", 0, 1));
    seg.onTranscript(makeSegment("How are you end dictation", 1, 2));

    expect(cb).toHaveBeenCalledTimes(1);
    expect(cb).toHaveBeenCalledWith(
      "Hello world How are you",
      expect.objectContaining({ startTime: 0, endTime: 2 }),
    );
  });

  it("emits on Chinese end phrase", () => {
    const seg = new DictationSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("今天天气很好", 0, 1));
    seg.onTranscript(makeSegment("明天也不错结束听写", 1, 2));

    expect(cb).toHaveBeenCalledTimes(1);
    expect(cb).toHaveBeenCalledWith(
      "今天天气很好 明天也不错",
      expect.objectContaining({ startTime: 0, endTime: 2 }),
    );
  });

  it("emits on flush", () => {
    const seg = new DictationSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("Hello world", 0, 1));
    const text = seg.flush();

    expect(text).toBe("Hello world");
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("returns null on flush with no buffer", () => {
    const seg = new DictationSegmentation();
    expect(seg.flush()).toBeNull();
  });

  it("ignores non-final transcripts", () => {
    const seg = new DictationSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript({ text: "partial", isFinal: false, start: 0, end: 1 });
    seg.onTranscript(makeSegment("end dictation", 1, 2));

    expect(cb).not.toHaveBeenCalled();
  });

  it("ignores VAD events", () => {
    const seg = new DictationSegmentation();
    // Should not throw
    seg.onVAD(true);
    seg.onVAD(false);
  });

  it("getBuffer returns current text without clearing", () => {
    const seg = new DictationSegmentation();
    seg.onTranscript(makeSegment("Hello", 0, 1));
    seg.onTranscript(makeSegment("World", 1, 2));

    expect(seg.getBuffer()).toBe("Hello World");

    // Buffer should still be there
    const cb = vi.fn();
    seg.onMessage(cb);
    seg.flush();
    expect(cb).toHaveBeenCalledWith("Hello World", expect.any(Object));
  });

  it("supports custom end phrases", () => {
    const seg = new DictationSegmentation({ endPhrases: ["stop recording"] });
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("Take note: buy milk", 0, 1));
    seg.onTranscript(makeSegment("and eggs stop recording", 1, 2));

    expect(cb).toHaveBeenCalledWith(
      "Take note: buy milk and eggs",
      expect.any(Object),
    );
  });

  it("clears buffer after emit", () => {
    const seg = new DictationSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("First batch end dictation", 0, 1));
    expect(cb).toHaveBeenCalledTimes(1);

    seg.onTranscript(makeSegment("Second batch end dictation", 2, 3));
    expect(cb).toHaveBeenCalledTimes(2);
    expect(cb).toHaveBeenLastCalledWith(
      "Second batch",
      expect.objectContaining({ startTime: 2, endTime: 3 }),
    );
  });
});
