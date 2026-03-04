import { z } from "zod";

export const VoiceConfigSchema = z.object({
  enabled: z.boolean().optional(),
  mode: z.enum(["conversation", "meeting", "dictation"]).optional(),
  audio: z
    .object({
      source: z.enum(["mic"]).optional(),
      sampleRate: z.number().optional(),
      device: z.string().optional(),
    })
    .optional(),
  stt: z
    .object({
      provider: z.string().optional(),
      model: z.string().optional(),
      languages: z.array(z.string()).optional(),
      apiKey: z.string().optional(),
      endpoint: z.string().optional(),
      extra: z.record(z.string(), z.string()).optional(),
    })
    .optional(),
  tts: z
    .object({
      enabled: z.boolean().optional(),
      provider: z.string().optional(),
      model: z.string().optional(),
      voice: z.string().optional(),
      apiKey: z.string().optional(),
      endpoint: z.string().optional(),
      format: z.string().optional(),
      sampleRate: z.number().optional(),
      speed: z.number().optional(),
      extra: z.record(z.string(), z.string()).optional(),
    })
    .optional(),
  conversation: z
    .object({
      endOfTurnSilence: z.number().optional(),
      interruptible: z.boolean().optional(),
    })
    .optional(),
  meeting: z
    .object({
      topicShiftThreshold: z.number().optional(),
      maxBlockDurationSec: z.number().optional(),
      silenceBlockMs: z.number().optional(),
      autoStopSilenceMs: z.number().optional(),
      agentKeywords: z.array(z.string()).optional(),
    })
    .optional(),
  dictation: z
    .object({
      endPhrases: z.array(z.string()).optional(),
    })
    .optional(),
});

export type VoiceConfig = z.infer<typeof VoiceConfigSchema>;
