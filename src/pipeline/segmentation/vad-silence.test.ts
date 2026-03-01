import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { TranscriptSegment, SegmentMetadata } from "../interfaces.js";
import { VADSilenceSegmentation } from "./vad-silence.js";

function makeFinal(text: string, start = 0, end = 1): TranscriptSegment {
  return { text, isFinal: true, start, end };
}

function makePartial(text: string, start = 0, end = 1): TranscriptSegment {
  return { text, isFinal: false, start, end };
}

describe("VADSilenceSegmentation", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("emits final segment immediately on onTranscript", () => {
    const seg = new VADSilenceSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeFinal("hello world", 0.0, 1.2));

    expect(cb).toHaveBeenCalledTimes(1);
    expect(cb).toHaveBeenCalledWith("hello world", {
      startTime: 0.0,
      endTime: 1.2,
      segmentCount: 1,
    });
  });

  it("ignores partial segments (isFinal: false)", () => {
    const seg = new VADSilenceSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makePartial("你"));
    seg.onTranscript(makePartial("你好"));
    seg.onTranscript(makePartial("你好世界"));

    expect(cb).not.toHaveBeenCalled();
  });

  it("emits only the final after a sequence of partials", () => {
    const seg = new VADSilenceSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makePartial("你"));
    seg.onTranscript(makePartial("你好"));
    seg.onTranscript(makeFinal("你好世界", 0.0, 2.0));

    expect(cb).toHaveBeenCalledTimes(1);
    expect(cb).toHaveBeenCalledWith("你好世界", {
      startTime: 0.0,
      endTime: 2.0,
      segmentCount: 1,
    });
  });

  it("emits each final segment independently", () => {
    const seg = new VADSilenceSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeFinal("first sentence", 0.0, 1.0));
    seg.onTranscript(makeFinal("second sentence", 1.0, 2.0));

    expect(cb).toHaveBeenCalledTimes(2);
    expect(cb).toHaveBeenNthCalledWith(1, "first sentence", {
      startTime: 0.0,
      endTime: 1.0,
      segmentCount: 1,
    });
    expect(cb).toHaveBeenNthCalledWith(2, "second sentence", {
      startTime: 1.0,
      endTime: 2.0,
      segmentCount: 1,
    });
  });

  it("skips empty final segments", () => {
    const seg = new VADSilenceSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeFinal("", 0.0, 0.5));
    seg.onTranscript(makeFinal("  ", 0.5, 1.0));

    expect(cb).not.toHaveBeenCalled();
  });

  it("VAD silence timer does not emit (segments not accumulated)", () => {
    const seg = new VADSilenceSegmentation({ silenceThresholdMs: 500 });
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onVAD(true);
    seg.onTranscript(makeFinal("hello", 0.0, 1.0));
    seg.onVAD(false);

    // cb already called once from onTranscript
    expect(cb).toHaveBeenCalledTimes(1);

    // Silence timer fires — should not produce another message
    vi.advanceTimersByTime(500);
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("flush returns null (nothing accumulated)", () => {
    const seg = new VADSilenceSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeFinal("already emitted", 0.0, 1.0));
    expect(cb).toHaveBeenCalledTimes(1);

    // flush has nothing left to emit
    const result = seg.flush();
    expect(result).toBeNull();
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("flush returns null when no segments at all", () => {
    const seg = new VADSilenceSegmentation();
    expect(seg.flush()).toBeNull();
  });
});
