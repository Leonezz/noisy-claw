import type { ChannelPlugin } from "openclaw/plugin-sdk";
import { voiceConfigAdapter, type ResolvedVoiceAccount } from "./config.js";
import { voiceGatewayAdapter } from "./gateway.js";
import { voiceOutboundAdapter } from "./outbound.js";

export const voiceChannelPlugin: ChannelPlugin<ResolvedVoiceAccount> = {
  id: "voice",

  meta: {
    id: "voice",
    label: "Voice",
    selectionLabel: "Voice (noisy-claw)",
    docsPath: "/channels/voice",
    docsLabel: "voice",
    blurb: "bidirectional voice channel; speak to your agent, hear it respond.",
    order: 80,
    quickstartAllowFrom: false,
  },

  capabilities: {
    chatTypes: ["direct"],
    polls: false,
    reactions: false,
    threads: false,
    media: false,
  },

  reload: { configPrefixes: ["channels.voice"] },

  config: voiceConfigAdapter,

  gateway: voiceGatewayAdapter,

  outbound: voiceOutboundAdapter,

  messaging: {
    normalizeTarget: (raw) => (raw.startsWith("voice:") ? raw : `voice:${raw}`),
    targetResolver: {
      looksLikeId: (raw) => raw.startsWith("voice:"),
      hint: "voice:<session-id>",
    },
  },
};
