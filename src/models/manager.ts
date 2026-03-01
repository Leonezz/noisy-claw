import { access } from "node:fs/promises";
import { join } from "node:path";
import { type ModelEntry, getRequiredModels, findModel, DEFAULT_STT_MODEL_ID } from "./catalog.js";
import { downloadModel, type DownloadProgress } from "./download.js";

export type EnsureModelsResult = {
  modelsDir: string;
  sttModelFilename: string;
  downloaded: string[];
  skipped: string[];
};

export type EnsureModelsOptions = {
  modelsDir: string;
  sttModelId?: string;
  sttProvider?: string;
  onProgress?: (progress: DownloadProgress) => void;
  onStatus?: (message: string) => void;
  signal?: AbortSignal;
};

async function fileExists(path: string): Promise<boolean> {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

export async function ensureModels(options: EnsureModelsOptions): Promise<EnsureModelsResult> {
  const { modelsDir, onProgress, onStatus, signal } = options;
  const sttProvider = options.sttProvider ?? "whisper";
  const sttModelId = options.sttModelId ?? DEFAULT_STT_MODEL_ID;

  // Collect required models (VAD is always needed)
  const needed: ModelEntry[] = [...getRequiredModels()];

  // Only include Whisper model when using local STT
  if (sttProvider === "whisper") {
    const sttModel = findModel(sttModelId);
    if (!sttModel) {
      throw new Error(`Unknown STT model: ${sttModelId}`);
    }
    if (!needed.some((m) => m.id === sttModel.id)) {
      needed.push(sttModel);
    }
  }

  const downloaded: string[] = [];
  const skipped: string[] = [];

  for (const model of needed) {
    const dest = join(modelsDir, model.filename);
    if (await fileExists(dest)) {
      skipped.push(model.id);
      continue;
    }

    onStatus?.(`Downloading ${model.description}...`);
    await downloadModel(model, dest, { onProgress, signal });
    downloaded.push(model.id);
  }

  // Resolve the STT model filename (empty string when using cloud)
  const sttModel = sttProvider === "whisper" ? findModel(sttModelId) : undefined;
  const sttModelFilename = sttModel?.filename ?? "";

  return {
    modelsDir,
    sttModelFilename,
    downloaded,
    skipped,
  };
}

/**
 * Resolve the models directory for the plugin.
 * Priority: NOISY_CLAW_MODELS_DIR env > stateDir/models
 */
export function resolveModelsDir(stateDir: string): string {
  if (process.env.NOISY_CLAW_MODELS_DIR) {
    return process.env.NOISY_CLAW_MODELS_DIR;
  }
  return join(stateDir, "models");
}
