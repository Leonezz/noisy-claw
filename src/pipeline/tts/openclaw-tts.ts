import type { TTSProvider, TTSOpts } from "../interfaces.js";

/**
 * Wraps OpenClaw's existing textToSpeech function.
 * The actual function is injected at construction time since the plugin
 * doesn't directly import core modules.
 */
export type TextToSpeechFn = (params: {
  text: string;
  cfg: unknown;
  channel?: string;
}) => Promise<{ success: boolean; audioPath?: string; error?: string }>;

export class OpenClawTTS implements TTSProvider {
  constructor(
    private readonly ttsFunction: TextToSpeechFn,
    private readonly config: unknown,
  ) {}

  async synthesize(text: string, _opts?: TTSOpts): Promise<string> {
    const result = await this.ttsFunction({
      text,
      cfg: this.config,
      channel: "voice",
    });

    if (!result.success || !result.audioPath) {
      throw new Error(result.error ?? "TTS synthesis failed");
    }

    return result.audioPath;
  }
}
