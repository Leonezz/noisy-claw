import { describe, it, expect } from "vitest";
import { parseEvent, serializeCommand, type Command, type AudioEvent } from "./protocol.js";

describe("serializeCommand", () => {
  it("serializes start_capture with defaults", () => {
    const cmd: Command = { cmd: "start_capture" };
    const json = serializeCommand(cmd);
    expect(JSON.parse(json)).toEqual({ cmd: "start_capture" });
  });

  it("serializes start_capture with options", () => {
    const cmd: Command = {
      cmd: "start_capture",
      device: "MacBook Pro Microphone",
      sample_rate: 44100,
    };
    const json = serializeCommand(cmd);
    const parsed = JSON.parse(json);
    expect(parsed.cmd).toBe("start_capture");
    expect(parsed.device).toBe("MacBook Pro Microphone");
    expect(parsed.sample_rate).toBe(44100);
  });

  it("serializes stop_capture", () => {
    const json = serializeCommand({ cmd: "stop_capture" });
    expect(JSON.parse(json)).toEqual({ cmd: "stop_capture" });
  });

  it("serializes play_audio", () => {
    const json = serializeCommand({ cmd: "play_audio", path: "/tmp/audio.mp3" });
    expect(JSON.parse(json)).toEqual({ cmd: "play_audio", path: "/tmp/audio.mp3" });
  });

  it("serializes shutdown", () => {
    const json = serializeCommand({ cmd: "shutdown" });
    expect(JSON.parse(json)).toEqual({ cmd: "shutdown" });
  });
});

describe("parseEvent", () => {
  it("parses ready event", () => {
    const event = parseEvent('{"event":"ready"}');
    expect(event).toEqual({ event: "ready" });
  });

  it("parses vad event", () => {
    const event = parseEvent('{"event":"vad","speaking":true}');
    expect(event).toEqual({ event: "vad", speaking: true });
  });

  it("parses transcript event", () => {
    const event = parseEvent(
      '{"event":"transcript","text":"hello world","is_final":true,"start":0.0,"end":1.2}',
    );
    expect(event).toEqual({
      event: "transcript",
      text: "hello world",
      is_final: true,
      start: 0.0,
      end: 1.2,
    });
  });

  it("parses transcript event with confidence", () => {
    const event = parseEvent(
      '{"event":"transcript","text":"hi","is_final":true,"start":0.0,"end":0.5,"confidence":0.95}',
    );
    expect(event).not.toBeNull();
    expect((event as any).confidence).toBe(0.95);
  });

  it("parses playback_done event", () => {
    const event = parseEvent('{"event":"playback_done"}');
    expect(event).toEqual({ event: "playback_done" });
  });

  it("parses status event", () => {
    const event = parseEvent('{"event":"status","capturing":true,"playing":false}');
    expect(event).toEqual({ event: "status", capturing: true, playing: false });
  });

  it("parses error event", () => {
    const event = parseEvent('{"event":"error","message":"device not found"}');
    expect(event).toEqual({ event: "error", message: "device not found" });
  });

  it("returns null for invalid JSON", () => {
    expect(parseEvent("not json")).toBeNull();
  });

  it("returns null for empty string", () => {
    expect(parseEvent("")).toBeNull();
  });
});

describe("protocol round-trip", () => {
  it("command serialization produces valid JSON", () => {
    const commands: Command[] = [
      { cmd: "start_capture" },
      { cmd: "stop_capture" },
      { cmd: "play_audio", path: "/tmp/test.mp3" },
      { cmd: "stop_playback" },
      { cmd: "get_status" },
      { cmd: "shutdown" },
    ];

    for (const cmd of commands) {
      const json = serializeCommand(cmd);
      const parsed = JSON.parse(json);
      expect(parsed.cmd).toBe(cmd.cmd);
    }
  });
});
