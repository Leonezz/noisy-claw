import { Type } from "@sinclair/typebox";
import type { VoiceMode } from "../channel/session.js";
import { switchMode } from "../channel/gateway.js";

export function createVoiceModeTool() {
  return {
    label: "Voice Mode",
    name: "voice_mode",
    description:
      "Switch the voice channel mode. " +
      "'conversation' for turn-based dialogue, 'meeting' for passive observation with topic-based segmentation, " +
      "'dictation' for continuous capture until end phrase.",
    parameters: Type.Object({
      mode: Type.Union(
        [Type.Literal("conversation"), Type.Literal("meeting"), Type.Literal("dictation")],
        { description: "The voice channel mode to switch to." },
      ),
    }),
    execute: async (_toolCallId: string, args: { mode: string }) => {
      const mode = args.mode as VoiceMode;
      switchMode(mode);

      return {
        content: [
          {
            type: "text" as const,
            text: `Voice mode set to '${mode}'.`,
          },
        ],
        details: { mode, applied: true },
      };
    },
  };
}
