import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { VoiceSession } from "../channel/session.js";
import { createVoiceModeTool } from "./voice-mode.js";
import { createVoiceStatusTool } from "./voice-status.js";

describe("createVoiceModeTool", () => {
  it("sets mode to conversation", async () => {
    const tool = createVoiceModeTool();

    const result = await tool.execute("call-1", { mode: "conversation" });
    expect(result.content[0].text).toContain("conversation");
    expect(result.details.applied).toBe(true);
  });

  it("accepts meeting mode", async () => {
    const tool = createVoiceModeTool();

    const result = await tool.execute("call-1", { mode: "meeting" });
    expect(result.content[0].text).toContain("meeting");
    expect(result.details.applied).toBe(true);
  });

  it("accepts dictation mode", async () => {
    const tool = createVoiceModeTool();

    const result = await tool.execute("call-1", { mode: "dictation" });
    expect(result.content[0].text).toContain("dictation");
    expect(result.details.applied).toBe(true);
  });

  it("has correct tool metadata", () => {
    const tool = createVoiceModeTool();

    expect(tool.name).toBe("voice_mode");
    expect(tool.label).toBe("Voice Mode");
    expect(tool.description).toBeTruthy();
    expect(tool.parameters).toBeTruthy();
  });
});

describe("createVoiceStatusTool", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-03-01T00:00:00Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns initial status", async () => {
    const session = new VoiceSession();
    const tool = createVoiceStatusTool(session);

    const result = await tool.execute("call-1");
    const status = JSON.parse(result.content[0].text);
    expect(status.active).toBe(false);
    expect(status.mode).toBe("conversation");
    expect(status.duration).toBe(0);
    expect(status.segmentCount).toBe(0);
  });

  it("returns active status with duration", async () => {
    const session = new VoiceSession();
    session.update(session.start());
    vi.advanceTimersByTime(3000);

    const tool = createVoiceStatusTool(session);
    const result = await tool.execute("call-1");
    const status = JSON.parse(result.content[0].text);

    expect(status.active).toBe(true);
    expect(status.duration).toBe(3);
    expect(status.currentlyListening).toBe(true);
  });

  it("includes details in result", async () => {
    const session = new VoiceSession();
    const tool = createVoiceStatusTool(session);

    const result = await tool.execute("call-1");
    expect(result.details).toBeTruthy();
    expect(result.details.active).toBe(false);
  });

  it("has correct tool metadata", () => {
    const session = new VoiceSession();
    const tool = createVoiceStatusTool(session);

    expect(tool.name).toBe("voice_status");
    expect(tool.label).toBe("Voice Status");
  });
});
