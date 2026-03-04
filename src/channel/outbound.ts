import type { ChannelOutboundAdapter } from "openclaw/plugin-sdk";
import { getActivePipeline, getActiveSession } from "./gateway.js";

export const voiceOutboundAdapter: ChannelOutboundAdapter = {
  deliveryMode: "direct",
  textChunkLimit: 4000,

  sendText: async (ctx) => {
    // Only synthesize TTS in conversation mode
    const session = getActiveSession();
    const mode = session?.getState().mode ?? "conversation";
    if (mode === "conversation") {
      const pipeline = getActivePipeline();
      if (pipeline) {
        pipeline.speak(ctx.text);
      }
    }

    return {
      channel: "voice",
      messageId: `voice-${Date.now()}`,
    };
  },

  sendMedia: async (ctx) => {
    const session = getActiveSession();
    const mode = session?.getState().mode ?? "conversation";
    if (ctx.text && mode === "conversation") {
      const pipeline = getActivePipeline();
      if (pipeline) {
        pipeline.speak(ctx.text);
      }
    }
    return {
      channel: "voice",
      messageId: `voice-${Date.now()}`,
    };
  },
};
