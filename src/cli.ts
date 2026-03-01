import { existsSync } from "node:fs";
import { join } from "node:path";
import type { Command } from "commander";
import { getSTTModels, DEFAULT_STT_MODEL_ID, findModel, MODEL_CATALOG } from "./models/catalog.js";
import { ensureModels } from "./models/manager.js";

export function registerVoiceCli(program: Command, stateDir: string): void {
  const voice = program.command("voice").description("Noisy Claw voice plugin commands");

  voice
    .command("setup")
    .description("Download required models for voice processing")
    .option("--stt-model <id>", "STT model to download", DEFAULT_STT_MODEL_ID)
    .option("--stt-provider <name>", "STT provider (whisper, aliyun, ...)", "whisper")
    .option("--tts-provider <name>", "TTS provider (aliyun, ...)")
    .action(async (opts: { sttModel: string; sttProvider: string; ttsProvider?: string }) => {
      const modelsDir = join(stateDir, "models");

      console.log("\n  Noisy Claw — Voice Setup\n");

      if (opts.sttProvider === "whisper") {
        const sttModels = getSTTModels();
        console.log("  STT provider: whisper (local)");
        console.log("  Available STT models:");
        for (const m of sttModels) {
          const marker = m.id === opts.sttModel ? " *" : "  ";
          const size = `${Math.round(m.sizeBytes / 1_000_000)}MB`;
          console.log(`  ${marker} ${m.id.padEnd(16)} ${size.padStart(6)}  ${m.description}`);
        }
        console.log(`\n  Selected: ${opts.sttModel}\n`);
      } else {
        console.log(`  STT provider: ${opts.sttProvider} (cloud)`);
        console.log("  Whisper model download skipped — using cloud STT.");
        console.log("  Ensure your API key is set (e.g. DASHSCOPE_API_KEY env var or in config).\n");
      }

      if (opts.ttsProvider) {
        console.log(`  TTS provider: ${opts.ttsProvider} (cloud)`);
        console.log("  Ensure your API key is set for TTS.\n");
      }

      const result = await ensureModels({
        modelsDir,
        sttModelId: opts.sttModel,
        sttProvider: opts.sttProvider,
        onProgress: (p) => {
          process.stdout.write(`\r  Downloading ${p.model.filename}... ${p.percent}%`);
        },
        onStatus: (msg) => console.log(`  ${msg}`),
      });

      if (result.downloaded.length > 0) {
        console.log(`\n  Downloaded: ${result.downloaded.join(", ")}`);
      }
      if (result.skipped.length > 0) {
        console.log(`  Already present: ${result.skipped.join(", ")}`);
      }
      console.log(`  Models dir: ${result.modelsDir}\n`);
    });

  voice
    .command("models")
    .description("List available and downloaded models")
    .action(async () => {
      const modelsDir = join(stateDir, "models");

      console.log("\n  Noisy Claw — Available Models\n");
      for (const m of MODEL_CATALOG) {
        const dest = join(modelsDir, m.filename);
        const present = existsSync(dest);
        const status = present ? "[downloaded]" : "[not downloaded]";
        const size = `${Math.round(m.sizeBytes / 1_000_000)}MB`;
        const required = m.required ? " (required)" : "";
        const isDefault = m.id === DEFAULT_STT_MODEL_ID ? " (default)" : "";
        console.log(`  ${m.id.padEnd(16)} ${size.padStart(6)}  ${status}${required}${isDefault}`);
        console.log(`    ${m.description}`);
      }
      console.log(`\n  Models dir: ${modelsDir}\n`);
    });
}
