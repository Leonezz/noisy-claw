import type { ChannelOutboundAdapter } from "openclaw/plugin-sdk";

export const voiceOutboundAdapter: ChannelOutboundAdapter = {
  deliveryMode: "direct",
  textChunkLimit: 4000,

  sendText: async (_ctx) => {
    // TTS is opt-in via the voice_speak tool; outbound adapter delivers text-only
    return {
      channel: "voice",
      messageId: `voice-${Date.now()}`,
    };
  },

  sendMedia: async (_ctx) => {
    // TTS is opt-in via the voice_speak tool; outbound adapter delivers text-only
    return {
      channel: "voice",
      messageId: `voice-${Date.now()}`,
    };
  },
};
