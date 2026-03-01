import { Type } from "@sinclair/typebox";
import type { VoiceSession } from "../channel/session.js";
import type { PipelineCoordinator } from "../pipeline/coordinator.js";

export type VoiceListenDeps = {
  session: VoiceSession;
  getPipeline: () => PipelineCoordinator | null;
};

export function createVoiceListenTool(deps: VoiceListenDeps) {
  return {
    label: "Voice Listen",
    name: "voice_listen",
    description:
      "Start or stop voice listening. When listening is true, the microphone is active " +
      "and speech is transcribed. When false, the microphone is paused. " +
      "Use this to control when the voice channel is capturing audio.",
    parameters: Type.Object({
      listening: Type.Boolean({
        description: "true to start listening, false to pause.",
      }),
    }),
    execute: async (_toolCallId: string, args: { listening: boolean }) => {
      const pipeline = deps.getPipeline();

      if (!pipeline) {
        return {
          content: [
            {
              type: "text" as const,
              text: "Voice channel is not active. Start the voice channel first.",
            },
          ],
          details: { listening: args.listening, applied: false },
        };
      }

      if (!pipeline.isActive) {
        return {
          content: [
            {
              type: "text" as const,
              text: "Voice pipeline is not running.",
            },
          ],
          details: { listening: args.listening, applied: false },
        };
      }

      if (args.listening) {
        pipeline.resume();
        deps.session.update(deps.session.setListening(true));
        return {
          content: [
            { type: "text" as const, text: "Voice listening resumed." },
          ],
          details: { listening: true, applied: true },
        };
      } else {
        pipeline.pause();
        deps.session.update(deps.session.setListening(false));
        return {
          content: [
            { type: "text" as const, text: "Voice listening paused." },
          ],
          details: { listening: false, applied: true },
        };
      }
    },
  };
}
