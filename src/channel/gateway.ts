import { existsSync } from "node:fs";
import type {
  ChannelGatewayAdapter,
  ChannelGatewayContext,
  PluginRuntime,
} from "openclaw/plugin-sdk";
import type { AudioEvent, SttConfig, TtsConfig } from "../ipc/protocol.js";
import { AudioSubprocess } from "../ipc/subprocess.js";
import { resolveModelsDir as resolveFromManager } from "../models/manager.js";
import { PipelineCoordinator, type PipelineComponents } from "../pipeline/coordinator.js";
import { RustLocalPlayback } from "../pipeline/output/rust-playback.js";
import { VADSilenceSegmentation } from "../pipeline/segmentation/vad-silence.js";
import { RustLocalCapture } from "../pipeline/sources/rust-capture.js";
import { RustWhisperSTT } from "../pipeline/stt/rust-whisper.js";
import type { ResolvedVoiceAccount } from "./config.js";
import { dispatchVoiceTranscript, type VoiceDispatchDeps } from "./dispatch.js";
import { VoiceSession } from "./session.js";

// Module-level state (accessible to tools and outbound adapter)
let activePipeline: PipelineCoordinator | null = null;
let activeSession: VoiceSession | null = null;
let activeSubprocess: AudioSubprocess | null = null;
let injectedRuntime: PluginRuntime | null = null;
let injectedStateDir: string | null = null;

export function getActivePipeline(): PipelineCoordinator | null {
  return activePipeline;
}

export function getActiveSession(): VoiceSession | null {
  return activeSession;
}

/**
 * Inject the plugin runtime at registration time.
 * Required for dispatching voice transcripts to the agent.
 */
export function setPluginRuntime(runtime: PluginRuntime): void {
  injectedRuntime = runtime;
}

/**
 * Inject the plugin state directory at service start time.
 * Used by resolveModelsDir to find downloaded models.
 */
export function setStateDir(dir: string): void {
  injectedStateDir = dir;
}

/**
 * Resolve the API key for a cloud provider config.
 * Priority: explicit config value > environment variable.
 */
function resolveApiKey(
  configKey: string | undefined,
  envVar: string,
): string | undefined {
  if (configKey) return configKey;
  return process.env[envVar];
}

export const voiceGatewayAdapter: ChannelGatewayAdapter<ResolvedVoiceAccount> = {
  startAccount: async (ctx: ChannelGatewayContext<ResolvedVoiceAccount>) => {
    console.log("[noisy-claw] startAccount called");
    const { account, abortSignal } = ctx;
    const config = account.config;
    console.log(`[noisy-claw] starting voice gateway, stt=${config.stt?.provider}, tts=${config.tts?.provider}`);

    const binaryPath = resolveBinaryPath();
    const modelsDir = resolveModelsDir();

    // Create session
    const session = new VoiceSession();
    activeSession = session;

    // Resolve STT config for IPC
    const sttProvider = config.stt?.provider ?? "whisper";
    const sttConfig: SttConfig | undefined =
      sttProvider !== "whisper"
        ? {
            provider: sttProvider,
            api_key: resolveApiKey(config.stt?.apiKey, "DASHSCOPE_API_KEY"),
            endpoint: config.stt?.endpoint,
            model: config.stt?.model,
            languages: config.stt?.languages ?? ["en"],
            extra: config.stt?.extra as Record<string, string> | undefined,
          }
        : undefined;

    // Resolve TTS config (used when speaking)
    const ttsProvider = config.tts?.provider;
    const ttsConfig: TtsConfig | undefined = ttsProvider
      ? {
          provider: ttsProvider,
          api_key: resolveApiKey(config.tts?.apiKey, "DASHSCOPE_API_KEY"),
          endpoint: config.tts?.endpoint,
          model: config.tts?.model,
          voice: config.tts?.voice,
          format: config.tts?.format,
          sample_rate: config.tts?.sampleRate,
          speed: config.tts?.speed,
          extra: config.tts?.extra as Record<string, string> | undefined,
        }
      : undefined;

    // Create pipeline components — these need the subprocess reference,
    // but the subprocess needs references to route events to them.
    // We create them first, then wire up event routing.
    let rustCapture: RustLocalCapture;
    let rustSTT: RustWhisperSTT;
    let rustPlayback: RustLocalPlayback;

    const subprocess = new AudioSubprocess({
      binaryPath,
      modelsDir,
      onEvent: (event: AudioEvent) => {
        if (event.event === "vad" || event.event === "transcript") {
          rustCapture?.handleEvent(event);
        }
        if (event.event === "transcript") {
          rustSTT?.handleEvent(event);
        }
        if (event.event === "playback_done" || event.event === "speak_done") {
          rustPlayback?.handleEvent(event);
        }
        if (event.event === "speak_started") {
          session.update(session.setSpeaking(true));
        }
        if (event.event === "speak_done") {
          session.update(session.setSpeaking(false));
        }
        if (event.event === "error") {
          console.error(
            `[noisy-claw] audio engine error: ${(event as { message?: string }).message}`,
          );
        }
      },
      onError: (err) => {
        console.error("[noisy-claw] subprocess error:", err);
      },
      onExit: (code) => {
        console.log(`[noisy-claw] subprocess exited with code ${code}`);
        activePipeline = null;
        activeSubprocess = null;
      },
    });

    // Now create the actual pipeline component instances
    rustCapture = new RustLocalCapture(subprocess);
    rustSTT = new RustWhisperSTT();
    rustPlayback = new RustLocalPlayback(subprocess);

    const segmentation = new VADSilenceSegmentation({
      silenceThresholdMs: config.conversation.endOfTurnSilence,
    });

    // Pass TTS config to playback — Rust subprocess handles TTS + playback in one step
    if (ttsConfig) {
      rustPlayback.setTtsConfig(ttsConfig);
    }

    const components: PipelineComponents = {
      audioSource: rustCapture,
      sttProvider: rustSTT,
      segmentation,
      audioOutput: rustPlayback,
    };

    const pipeline = new PipelineCoordinator(components);
    activePipeline = pipeline;
    activeSubprocess = subprocess;

    // Wire message callback to track segments and dispatch to agent
    pipeline.onMessage((message, metadata) => {
      session.update(session.incrementSegments());

      if (!injectedRuntime) {
        console.warn("[noisy-claw] transcript received but no runtime injected, skipping dispatch");
        return;
      }

      const deps: VoiceDispatchDeps = {
        runtime: injectedRuntime,
        cfg: ctx.cfg as unknown as Record<string, unknown>,
        accountId: account.accountId,
        getPipeline: getActivePipeline,
      };

      console.log(
        `[noisy-claw] dispatching transcript: "${message.slice(0, 80)}${message.length > 80 ? "..." : ""}"`,
      );
      void dispatchVoiceTranscript(deps, message, metadata).catch((err) => {
        console.error("[noisy-claw] failed to dispatch transcript:", err);
      });
    });

    // Start the subprocess (begins listening for IPC)
    subprocess.start();

    // Start the pipeline — begins audio capture and STT
    pipeline.start({
      audio: {
        device: config.audio.device ?? "default",
        sampleRate: config.audio.sampleRate ?? 16000,
      },
      stt: {
        model: config.stt?.model ?? "base",
        language: config.stt?.languages?.[0] ?? "en",
      },
      sttConfig,
    });

    // Mark session as active and listening
    session.update(session.start());

    ctx.setStatus({
      accountId: account.accountId,
      name: "Voice (active)",
      connected: true,
      running: true,
    });

    // Wait for abort signal — keeps the gateway alive
    await new Promise<void>((resolve) => {
      abortSignal.addEventListener(
        "abort",
        () => {
          subprocess.stop();
          pipeline.stop();
          session.update(session.stop());
          activePipeline = null;
          activeSession = null;
          activeSubprocess = null;
          resolve();
        },
        { once: true },
      );
    });
  },

  stopAccount: async () => {
    activeSubprocess?.stop();
    activePipeline?.stop();
    activePipeline = null;
    activeSession = null;
    activeSubprocess = null;
  },
};

function resolveBinaryPath(): string {
  // In development: cargo build output
  // In production: bundled binary next to the extension
  const devPath = new URL(
    "../../native/noisy-claw-audio/target/release/noisy-claw-audio",
    import.meta.url,
  ).pathname;

  return devPath;
}

function resolveModelsDir(): string {
  // In dev: check if repo models/ directory exists
  const devPath = new URL("../../models", import.meta.url).pathname;
  if (existsSync(devPath)) {
    return devPath;
  }

  // Production: use manager's resolution (env var > state dir)
  if (injectedStateDir) {
    return resolveFromManager(injectedStateDir);
  }

  // Fallback to dev path even if it doesn't exist yet
  return devPath;
}
