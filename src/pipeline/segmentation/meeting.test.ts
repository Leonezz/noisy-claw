import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { MeetingSegmentation } from "./meeting.js";
import type { TranscriptSegment, SegmentMetadata } from "../interfaces.js";

function makeSegment(text: string, start = 0, end = 1): TranscriptSegment {
  return { text, isFinal: true, start, end };
}

describe("MeetingSegmentation", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("accumulates transcripts without emitting", () => {
    const seg = new MeetingSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("Hello world"));
    seg.onTranscript(makeSegment("Another sentence"));

    expect(cb).not.toHaveBeenCalled();
  });

  it("emits on topic shift", () => {
    const seg = new MeetingSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("Topic A content", 0, 1));
    seg.onTranscript(makeSegment("More about topic A", 1, 2));
    seg.onTopicShift();

    expect(cb).toHaveBeenCalledTimes(1);
    expect(cb).toHaveBeenCalledWith(
      "Topic A content More about topic A",
      expect.objectContaining({ startTime: 0, endTime: 2 }),
    );
  });

  it("emits on silence timeout", () => {
    const seg = new MeetingSegmentation({ silenceBlockMs: 1000 });
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("Some text"));
    seg.onVAD(false);

    vi.advanceTimersByTime(1000);
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("cancels silence timer on speech resume", () => {
    const seg = new MeetingSegmentation({ silenceBlockMs: 1000 });
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("Some text"));
    seg.onVAD(false);
    vi.advanceTimersByTime(500);
    seg.onVAD(true);
    vi.advanceTimersByTime(1000);

    expect(cb).not.toHaveBeenCalled();
  });

  it("emits on flush", () => {
    const seg = new MeetingSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("Buffer content", 0, 1));
    const text = seg.flush();

    expect(text).toBe("Buffer content");
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("detects keyword and fires callback", () => {
    const seg = new MeetingSegmentation({ agentKeywords: ["molty", "助手"] });
    const keywordCb = vi.fn();
    seg.onKeyword(keywordCb);

    seg.onTranscript(makeSegment("Hey molty, what time is it?"));

    expect(keywordCb).toHaveBeenCalledWith("Hey molty, what time is it?");
  });

  it("detects Chinese keyword", () => {
    const seg = new MeetingSegmentation({ agentKeywords: ["助手"] });
    const keywordCb = vi.fn();
    seg.onKeyword(keywordCb);

    seg.onTranscript(makeSegment("助手请帮我查一下"));

    expect(keywordCb).toHaveBeenCalledWith("助手请帮我查一下");
  });

  it("getBuffer returns current text without clearing", () => {
    const seg = new MeetingSegmentation();
    seg.onTranscript(makeSegment("First"));
    seg.onTranscript(makeSegment("Second"));

    expect(seg.getBuffer()).toBe("First Second");

    // Should still be there for flush
    const cb = vi.fn();
    seg.onMessage(cb);
    seg.flush();
    expect(cb).toHaveBeenCalledWith("First Second", expect.any(Object));
  });

  it("ignores non-final transcripts", () => {
    const seg = new MeetingSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript({ text: "partial", isFinal: false, start: 0, end: 1 });
    seg.onTopicShift();

    expect(cb).not.toHaveBeenCalled();
  });

  it("auto-stop fires callback after prolonged silence", () => {
    const seg = new MeetingSegmentation({ autoStopSilenceMs: 2000, silenceBlockMs: 1000 });
    const autoStopCb = vi.fn();
    seg.onAutoStop(autoStopCb);
    const msgCb = vi.fn();
    seg.onMessage(msgCb);

    seg.onTranscript(makeSegment("Some content"));
    seg.onVAD(false);

    vi.advanceTimersByTime(2000);
    expect(autoStopCb).toHaveBeenCalledTimes(1);
  });

  it("clears buffer after emit", () => {
    const seg = new MeetingSegmentation();
    const cb = vi.fn();
    seg.onMessage(cb);

    seg.onTranscript(makeSegment("Block 1", 0, 1));
    seg.onTopicShift();
    seg.onTranscript(makeSegment("Block 2", 2, 3));
    seg.onTopicShift();

    expect(cb).toHaveBeenCalledTimes(2);
    expect(cb).toHaveBeenLastCalledWith("Block 2", expect.objectContaining({ startTime: 2 }));
  });
});
