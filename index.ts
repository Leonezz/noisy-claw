import type { OpenClawPluginApi } from "openclaw/plugin-sdk";
import { emptyPluginConfigSchema } from "openclaw/plugin-sdk";
import { getActivePipeline, getActiveSession, setPluginRuntime, setStateDir } from "./src/channel/gateway.js";
import { voiceChannelPlugin } from "./src/channel/plugin.js";
import { VoiceSession } from "./src/channel/session.js";
import { registerVoiceCli } from "./src/cli.js";
import { ensureModels, resolveModelsDir } from "./src/models/manager.js";
import { createVoiceListenTool } from "./src/tools/voice-listen.js";
import { createVoiceModeTool } from "./src/tools/voice-mode.js";
import { createVoiceStatusTool } from "./src/tools/voice-status.js";

// Fallback session used when the gateway hasn't started yet.
const fallbackSession = new VoiceSession();

const plugin = {
  id: "noisy-claw",
  name: "Noisy Claw",
  description: "Voice channel plugin — bidirectional voice as a first-class channel",
  configSchema: emptyPluginConfigSchema(),

  register(api: OpenClawPluginApi) {
    // Inject the runtime so the gateway can dispatch transcripts to the agent
    setPluginRuntime(api.runtime);

    // Set state dir early (before gateway/service start) so resolveModelsDir() works
    setStateDir(api.runtime.state.resolveStateDir());

    // Register the voice channel
    api.registerChannel(voiceChannelPlugin);

    // Register agent tools.
    // Tools use the gateway's active session when available,
    // falling back to an idle session before the channel starts.
    api.registerTool(() => createVoiceModeTool(getActiveSession() ?? fallbackSession));
    api.registerTool(() => createVoiceStatusTool(getActiveSession() ?? fallbackSession));
    api.registerTool(() =>
      createVoiceListenTool({
        session: getActiveSession() ?? fallbackSession,
        getPipeline: getActivePipeline,
      }),
    );

    // CLI: `openclaw voice setup` / `openclaw voice models`
    api.registerCli(
      ({ program }) => {
        const stateDir = api.runtime.state.resolveStateDir();
        registerVoiceCli(program, stateDir);
      },
      { commands: ["voice"] },
    );

    // Service: auto-download missing models at gateway startup
    api.registerService({
      id: "noisy-claw-models",
      start: async (ctx) => {
        const modelsDir = resolveModelsDir(ctx.stateDir);

        // If NOISY_CLAW_STT_PROVIDER is set, skip local Whisper download
        const sttProvider = process.env.NOISY_CLAW_STT_PROVIDER ?? "whisper";

        const result = await ensureModels({
          modelsDir,
          sttProvider,
          onStatus: (msg) => ctx.logger.info(msg),
          onProgress: (p) => {
            if (p.percent % 25 === 0) {
              ctx.logger.info(`${p.model.filename}: ${p.percent}%`);
            }
          },
        });
        if (result.downloaded.length > 0) {
          ctx.logger.info(`Downloaded models: ${result.downloaded.join(", ")}`);
        }
      },
    });
  },
};

export default plugin;
