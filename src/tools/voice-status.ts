import { Type } from "@sinclair/typebox";
import type { VoiceSession } from "../channel/session.js";

export function createVoiceStatusTool(session: VoiceSession) {
  return {
    label: "Voice Status",
    name: "voice_status",
    description: "Get the current state of the voice channel session.",
    parameters: Type.Object({}),
    execute: async (_toolCallId: string) => {
      const state = session.getState();

      const status = {
        active: state.active,
        mode: state.mode,
        duration: session.getDuration(),
        segmentCount: state.segmentCount,
        currentlyListening: state.currentlyListening,
        currentlySpeaking: state.currentlySpeaking,
      };

      return {
        content: [{ type: "text" as const, text: JSON.stringify(status, null, 2) }],
        details: status,
      };
    },
  };
}
