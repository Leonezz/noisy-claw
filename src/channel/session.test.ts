import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { VoiceSession } from "./session.js";

describe("VoiceSession", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-03-01T00:00:00Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("initial state is inactive", () => {
    const session = new VoiceSession();
    const state = session.getState();
    expect(state.active).toBe(false);
    expect(state.mode).toBe("conversation");
    expect(state.startTime).toBeNull();
    expect(state.segmentCount).toBe(0);
    expect(state.currentlyListening).toBe(false);
    expect(state.currentlySpeaking).toBe(false);
  });

  it("start() returns active state with timestamp", () => {
    const session = new VoiceSession();
    const next = session.start();
    expect(next.active).toBe(true);
    expect(next.startTime).toBe(Date.now());
    expect(next.currentlyListening).toBe(true);
    expect(next.segmentCount).toBe(0);
  });

  it("stop() returns inactive state", () => {
    const session = new VoiceSession();
    session.update(session.start());
    const next = session.stop();
    expect(next.active).toBe(false);
    expect(next.startTime).toBeNull();
    expect(next.currentlyListening).toBe(false);
    expect(next.currentlySpeaking).toBe(false);
  });

  it("setMode() returns state with new mode", () => {
    const session = new VoiceSession();
    const next = session.setMode("listen");
    expect(next.mode).toBe("listen");
  });

  it("incrementSegments() increments by 1", () => {
    const session = new VoiceSession();
    session.update(session.start());
    session.update(session.incrementSegments());
    session.update(session.incrementSegments());
    expect(session.getState().segmentCount).toBe(2);
  });

  it("setSpeaking() updates speaking flag", () => {
    const session = new VoiceSession();
    const next = session.setSpeaking(true);
    expect(next.currentlySpeaking).toBe(true);
  });

  it("setListening() updates listening flag", () => {
    const session = new VoiceSession();
    const next = session.setListening(true);
    expect(next.currentlyListening).toBe(true);
  });

  it("getDuration() returns 0 when not started", () => {
    const session = new VoiceSession();
    expect(session.getDuration()).toBe(0);
  });

  it("getDuration() returns elapsed seconds", () => {
    const session = new VoiceSession();
    session.update(session.start());
    vi.advanceTimersByTime(5000);
    expect(session.getDuration()).toBe(5);
  });

  it("update() applies state immutably", () => {
    const session = new VoiceSession();
    const before = session.getState();
    session.update(session.start());
    const after = session.getState();
    // Original state object is unchanged
    expect(before.active).toBe(false);
    expect(after.active).toBe(true);
  });

  it("start() resets segment count", () => {
    const session = new VoiceSession();
    session.update(session.start());
    session.update(session.incrementSegments());
    session.update(session.incrementSegments());
    expect(session.getState().segmentCount).toBe(2);
    // Restart resets
    session.update(session.start());
    expect(session.getState().segmentCount).toBe(0);
  });
});
