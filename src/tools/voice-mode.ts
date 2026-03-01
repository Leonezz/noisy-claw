import { Type } from "@sinclair/typebox";
import type { VoiceSession, VoiceMode } from "../channel/session.js";

export function createVoiceModeTool(session: VoiceSession) {
  return {
    label: "Voice Mode",
    name: "voice_mode",
    description:
      "Switch the voice channel mode. Only 'conversation' is currently supported. " +
      "'listen' and 'dictation' modes will be available in a future release.",
    parameters: Type.Object({
      mode: Type.Union(
        [Type.Literal("conversation"), Type.Literal("listen"), Type.Literal("dictation")],
        { description: "The voice channel mode to switch to." },
      ),
    }),
    execute: async (_toolCallId: string, args: { mode: string }) => {
      const mode = args.mode as VoiceMode;

      if (mode !== "conversation") {
        return {
          content: [
            {
              type: "text" as const,
              text: `Mode '${mode}' is not yet implemented. Only 'conversation' mode is available.`,
            },
          ],
          details: { mode, applied: false },
        };
      }

      session.update(session.setMode(mode));

      return {
        content: [
          {
            type: "text" as const,
            text: `Voice channel mode set to '${mode}'.`,
          },
        ],
        details: { mode, applied: true },
      };
    },
  };
}
