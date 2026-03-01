import type { VoiceConfig } from "./schema.js";

export const DEFAULT_VOICE_CONFIG: Required<VoiceConfig> = {
  enabled: true,
  mode: "conversation",
  audio: {
    source: "mic",
    sampleRate: 16000,
    device: "default",
  },
  stt: {
    backend: "whisper",
    model: "base",
    language: "en",
  },
  tts: {
    enabled: true,
  },
  conversation: {
    endOfTurnSilence: 700,
    interruptible: true,
  },
};
