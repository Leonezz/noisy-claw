import { Type } from "@sinclair/typebox";
import type { PipelineCoordinator } from "../pipeline/coordinator.js";
import type { VoiceSession, TranscriptEntry } from "../channel/session.js";

export type VoiceTranscriptDeps = {
  getPipeline: () => PipelineCoordinator | null;
  getSession: () => VoiceSession | null;
};

function formatHistory(entries: readonly TranscriptEntry[]): string {
  if (entries.length === 0) return "(no transcript history)";

  return entries
    .map((e, i) => {
      const time = new Date(e.timestamp).toLocaleTimeString("en-US", { hour12: false });
      return `[${i + 1}] (${time}, ${e.mode}) ${e.text}`;
    })
    .join("\n\n");
}

export function createVoiceTranscriptTool(deps: VoiceTranscriptDeps) {
  return {
    label: "Voice Transcript",
    name: "voice_transcript",
    description:
      "Read, flush, or retrieve history of transcripts. In dictation and meeting modes, " +
      "text accumulates in a buffer. Use 'get' to read without clearing, 'flush' to read and clear, " +
      "'history' to get the full session transcript log across all emitted blocks.",
    parameters: Type.Object({
      action: Type.Union(
        [Type.Literal("get"), Type.Literal("flush"), Type.Literal("history")],
        {
          description:
            "'get' reads the current buffer without clearing, " +
            "'flush' reads and clears the buffer, " +
            "'history' returns all emitted transcript blocks from this session.",
        },
      ),
    }),
    execute: async (_toolCallId: string, args: { action: string }) => {
      const pipeline = deps.getPipeline();
      const session = deps.getSession();

      if (args.action === "history") {
        const entries = session?.getTranscriptHistory() ?? [];
        const text = formatHistory(entries);
        return {
          content: [{ type: "text" as const, text }],
          details: { action: "history", entryCount: entries.length },
        };
      }

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
