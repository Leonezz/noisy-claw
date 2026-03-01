import { z } from "zod";

export const VoiceConfigSchema = z.object({
  enabled: z.boolean().optional(),
  mode: z.enum(["conversation", "listen", "dictation"]).optional(),
  audio: z
    .object({
      source: z.enum(["mic"]).optional(),
      sampleRate: z.number().optional(),
      device: z.string().optional(),
    })
    .optional(),
  stt: z
    .object({
      backend: z.enum(["whisper"]).optional(),
      model: z.string().optional(),
      language: z.string().optional(),
    })
    .optional(),
  tts: z
    .object({
      enabled: z.boolean().optional(),
    })
    .optional(),
  conversation: z
    .object({
      endOfTurnSilence: z.number().optional(),
      interruptible: z.boolean().optional(),
    })
    .optional(),
});

export type VoiceConfig = z.infer<typeof VoiceConfigSchema>;
