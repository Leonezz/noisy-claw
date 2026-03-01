import type { ChannelConfigAdapter } from "openclaw/plugin-sdk";
import { DEFAULT_VOICE_CONFIG } from "../config/defaults.js";
import type { VoiceConfig } from "../config/schema.js";

export type ResolvedVoiceAccount = {
  accountId: string;
  config: Required<VoiceConfig>;
};

export const voiceConfigAdapter: ChannelConfigAdapter<ResolvedVoiceAccount> = {
  listAccountIds: () => {
    // Voice channel has a single implicit account
    return ["default"];
  },

  resolveAccount: (cfg, accountId) => {
    const voiceCfg = (cfg as Record<string, unknown>).channels as
      | Record<string, unknown>
      | undefined;
    const raw = (voiceCfg?.voice ?? {}) as Partial<VoiceConfig>;
    return {
      accountId: accountId ?? "default",
      config: {
        ...DEFAULT_VOICE_CONFIG,
        ...raw,
        audio: { ...DEFAULT_VOICE_CONFIG.audio, ...raw.audio },
        stt: { ...DEFAULT_VOICE_CONFIG.stt, ...raw.stt },
        tts: { ...DEFAULT_VOICE_CONFIG.tts, ...raw.tts },
        conversation: {
          ...DEFAULT_VOICE_CONFIG.conversation,
          ...raw.conversation,
        },
      },
    };
  },

  defaultAccountId: () => "default",

  isEnabled: (account) => {
    return account.config.enabled !== false;
  },

  isConfigured: () => true, // No external service to configure

  describeAccount: (account) => ({
    accountId: account.accountId,
    name: "Voice (local mic)",
    connected: true,
    configured: true,
    enabled: account.config.enabled !== false,
  }),
};
