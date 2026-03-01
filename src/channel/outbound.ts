import type { ChannelOutboundAdapter } from "openclaw/plugin-sdk";
import { getActivePipeline } from "./gateway.js";

export const voiceOutboundAdapter: ChannelOutboundAdapter = {
  deliveryMode: "direct",
  textChunkLimit: 4000,

  sendText: async (ctx) => {
    // Text is already delivered via the normal channel path.
    // Additionally, synthesize and play audio.
    const pipeline = getActivePipeline();
    if (pipeline) {
      try {
        await pipeline.speak(ctx.text);
      } catch (err) {
        console.error("[noisy-claw] TTS playback failed:", err);
      }
    }

    return {
      channel: "voice",
      messageId: `voice-${Date.now()}`,
    };
  },

  sendMedia: async (ctx) => {
    // Voice channel doesn't support media — just read the text caption
    if (ctx.text) {
      const pipeline = getActivePipeline();
      if (pipeline) {
        try {
          await pipeline.speak(ctx.text);
        } catch (err) {
          console.error("[noisy-claw] TTS playback failed:", err);
        }
      }
    }
    return {
      channel: "voice",
      messageId: `voice-${Date.now()}`,
    };
  },
};
