import { describe, it, expect } from "vitest";
import { DEFAULT_VOICE_CONFIG } from "./defaults.js";
import { VoiceConfigSchema } from "./schema.js";

describe("VoiceConfigSchema", () => {
  it("accepts empty object (all optional)", () => {
    const result = VoiceConfigSchema.safeParse({});
    expect(result.success).toBe(true);
  });

  it("accepts full default config", () => {
    const result = VoiceConfigSchema.safeParse(DEFAULT_VOICE_CONFIG);
    expect(result.success).toBe(true);
  });

  it("accepts partial config", () => {
    const result = VoiceConfigSchema.safeParse({
      enabled: false,
      audio: { device: "MacBook Pro Microphone" },
    });
    expect(result.success).toBe(true);
  });

  it("rejects invalid mode", () => {
    const result = VoiceConfigSchema.safeParse({ mode: "invalid" });
    expect(result.success).toBe(false);
  });

  it("rejects invalid audio source", () => {
    const result = VoiceConfigSchema.safeParse({
      audio: { source: "file" },
    });
    expect(result.success).toBe(false);
  });

  it("rejects invalid stt backend", () => {
    const result = VoiceConfigSchema.safeParse({
      stt: { backend: "deepgram" },
    });
    expect(result.success).toBe(false);
  });
});

describe("DEFAULT_VOICE_CONFIG", () => {
  it("has all required fields", () => {
    expect(DEFAULT_VOICE_CONFIG.enabled).toBe(true);
    expect(DEFAULT_VOICE_CONFIG.mode).toBe("conversation");
    expect(DEFAULT_VOICE_CONFIG.audio.sampleRate).toBe(16000);
    expect(DEFAULT_VOICE_CONFIG.audio.device).toBe("default");
    expect(DEFAULT_VOICE_CONFIG.stt.backend).toBe("whisper");
    expect(DEFAULT_VOICE_CONFIG.stt.language).toBe("en");
    expect(DEFAULT_VOICE_CONFIG.tts.enabled).toBe(true);
    expect(DEFAULT_VOICE_CONFIG.conversation.endOfTurnSilence).toBe(700);
    expect(DEFAULT_VOICE_CONFIG.conversation.interruptible).toBe(true);
  });
});
