import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { TranscriptSegment, SegmentMetadata } from "../interfaces.js";
import { VADSilenceSegmentation } from "./vad-silence.js";

function makeSegment(text: string, start = 0, end = 1): TranscriptSegment {
  return { text, isFinal: true, start, end };
}

describe("VADSilenceSegmentation", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("emits nothing when no segments accumulated", () => {
    const seg = new VADSilenceSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    // User speaks then stops
    seg.onVAD(true);
    seg.onVAD(false);
    vi.advanceTimersByTime(700);

    expect(cb).not.toHaveBeenCalled();
  });

  it("emits message after silence threshold", () => {
    const seg = new VADSilenceSegmentation({ silenceThresholdMs: 500 });
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onVAD(true);
    seg.onTranscript(makeSegment("hello world", 0.0, 1.2));
    seg.onVAD(false);

    // Not yet — 499ms
    vi.advanceTimersByTime(499);
    expect(cb).not.toHaveBeenCalled();

    // Now at 500ms — should emit
    vi.advanceTimersByTime(1);
    expect(cb).toHaveBeenCalledTimes(1);
    expect(cb).toHaveBeenCalledWith("hello world", {
      startTime: 0.0,
      endTime: 1.2,
      segmentCount: 1,
    });
  });

  it("concatenates multiple segments into one turn", () => {
    const seg = new VADSilenceSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onVAD(true);
    seg.onTranscript(makeSegment("hello", 0.0, 0.5));
    seg.onTranscript(makeSegment("world", 0.5, 1.0));
    seg.onVAD(false);

    vi.advanceTimersByTime(700);

    expect(cb).toHaveBeenCalledTimes(1);
    expect(cb).toHaveBeenCalledWith("hello world", {
      startTime: 0.0,
      endTime: 1.0,
      segmentCount: 2,
    });
  });

  it("cancels timer when user resumes speaking", () => {
    const seg = new VADSilenceSegmentation({ silenceThresholdMs: 500 });
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onVAD(true);
    seg.onTranscript(makeSegment("hello", 0.0, 0.5));
    seg.onVAD(false);

    // Partial silence
    vi.advanceTimersByTime(300);
    expect(cb).not.toHaveBeenCalled();

    // User resumes speaking — timer should be cancelled
    seg.onVAD(true);
    seg.onTranscript(makeSegment("again", 0.8, 1.3));
    seg.onVAD(false);

    // Full silence after resume
    vi.advanceTimersByTime(500);
    expect(cb).toHaveBeenCalledTimes(1);
    expect(cb).toHaveBeenCalledWith("hello again", {
      startTime: 0.0,
      endTime: 1.3,
      segmentCount: 2,
    });
  });

  it("flush returns accumulated text immediately", () => {
    const seg = new VADSilenceSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onVAD(true);
    seg.onTranscript(makeSegment("flushed text", 0.0, 1.0));

    const result = seg.flush();
    expect(result).toBe("flushed text");
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("flush returns null when no segments", () => {
    const seg = new VADSilenceSegmentation();
    expect(seg.flush()).toBeNull();
  });

  it("resets segments after emitting", () => {
    const seg = new VADSilenceSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("first turn"));
    seg.onVAD(true);
    seg.onVAD(false);
    vi.advanceTimersByTime(700);

    expect(cb).toHaveBeenCalledTimes(1);

    // Second turn
    seg.onTranscript(makeSegment("second turn"));
    seg.onVAD(true);
    seg.onVAD(false);
    vi.advanceTimersByTime(700);

    expect(cb).toHaveBeenCalledTimes(2);
    expect(cb).toHaveBeenLastCalledWith(
      "second turn",
      expect.objectContaining({ segmentCount: 1 }),
    );
  });

  it("uses default 700ms threshold", () => {
    const seg = new VADSilenceSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("test"));
    seg.onVAD(false);

    vi.advanceTimersByTime(699);
    expect(cb).not.toHaveBeenCalled();

    vi.advanceTimersByTime(1);
    expect(cb).toHaveBeenCalledTimes(1);
  });
});
