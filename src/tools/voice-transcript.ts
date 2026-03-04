import { Type } from "@sinclair/typebox";
import type { PipelineCoordinator } from "../pipeline/coordinator.js";

export type VoiceTranscriptDeps = {
  getPipeline: () => PipelineCoordinator | null;
};

export function createVoiceTranscriptTool(deps: VoiceTranscriptDeps) {
  return {
    label: "Voice Transcript",
    name: "voice_transcript",
    description:
      "Read or flush the current transcript buffer. In dictation and meeting modes, " +
      "text accumulates in a buffer. Use 'get' to read without clearing, 'flush' to read and clear.",
    parameters: Type.Object({
      action: Type.Union([Type.Literal("get"), Type.Literal("flush")], {
        description: "'get' reads the buffer without clearing, 'flush' reads and clears it.",
      }),
    }),
    execute: async (_toolCallId: string, args: { action: string }) => {
      const pipeline = deps.getPipeline();

      if (!pipeline?.isActive) {
        return {
          content: [
            {
              type: "text" as const,
              text: "Voice channel is not active.",
            },
          ],
          details: { action: args.action, text: null },
        };
      }

      const text =
        args.action === "flush"
          ? pipeline.flushTranscript() ?? ""
          : pipeline.getTranscriptBuffer();

      return {
        content: [
          {
            type: "text" as const,
            text: text || "(empty buffer)",
          },
        ],
        details: { action: args.action, text, length: text.length },
      };
    },
  };
}
