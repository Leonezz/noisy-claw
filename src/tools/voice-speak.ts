import { Type } from "@sinclair/typebox";
import type { PipelineCoordinator } from "../pipeline/coordinator.js";

export type VoiceSpeakDeps = {
  getPipeline: () => PipelineCoordinator | null;
};

export function createVoiceSpeakTool(deps: VoiceSpeakDeps) {
  return {
    label: "Voice Speak",
    name: "voice_speak",
    description:
      "Speak text aloud through the voice channel using text-to-speech. " +
      "Use this to read out information, respond verbally, or announce something to the user.",
    parameters: Type.Object({
      text: Type.String({
        description: "The text to speak aloud.",
      }),
    }),
    execute: async (_toolCallId: string, args: { text: string }) => {
      const pipeline = deps.getPipeline();

      if (!pipeline?.isActive) {
        return {
          content: [
            {
              type: "text" as const,
              text: "Voice channel is not active. Cannot speak.",
            },
          ],
          details: { spoken: false },
        };
      }

      const textToSpeak = args.text.trim();
      if (!textToSpeak) {
        return {
          content: [
            { type: "text" as const, text: "No text provided to speak." },
          ],
          details: { spoken: false },
        };
      }

      pipeline.speak(textToSpeak);

      return {
        content: [
          { type: "text" as const, text: `Spoken: "${textToSpeak.slice(0, 80)}${textToSpeak.length > 80 ? "…" : ""}"` },
        ],
        details: { spoken: true, length: textToSpeak.length },
      };
    },
  };
}
