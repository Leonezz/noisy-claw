import type { ChannelConfigAdapter } from "openclaw/plugin-sdk";
import { DEFAULT_VOICE_CONFIG } from "../config/defaults.js";
import { VoiceConfigSchema, type VoiceConfig } from "../config/schema.js";

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
    const rawInput = voiceCfg?.voice ?? {};

    const parsed = VoiceConfigSchema.safeParse(rawInput);
    if (!parsed.success) {
      console.warn(
        `[noisy-claw] invalid voice config, using defaults: ${parsed.error.message}`,
      );
    }
    const raw: Partial<VoiceConfig> = parsed.success ? parsed.data : {};

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

  isConfigured: (account) => {
    const stt = account.config.stt?.provider ?? "whisper";
    if (stt !== "whisper") {
      const hasKey = !!(
        account.config.stt?.apiKey || process.env.DASHSCOPE_API_KEY
      );
      if (!hasKey) return false;
    }
    return true;
  },

  describeAccount: (account) => {
    const stt = account.config.stt?.provider ?? "whisper";
    const tts = account.config.tts?.provider;
    const isCloud = stt !== "whisper" || !!tts;
    const hasApiKey = !!(
      account.config.stt?.apiKey ||
      account.config.tts?.apiKey ||
      process.env.DASHSCOPE_API_KEY
    );
    const configured = !isCloud || hasApiKey;

    const label = isCloud
      ? `Voice (${stt}${tts ? ` + ${tts} TTS` : ""})`
      : "Voice (local mic)";

    return {
      accountId: account.accountId,
      name: label,
      connected: configured,
      configured,
      enabled: account.config.enabled !== false,
    };
  },
};
