import { createWriteStream } from "node:fs";
import { mkdir, rename, stat, unlink } from "node:fs/promises";
import { dirname } from "node:path";
import type { ModelEntry } from "./catalog.js";

export type DownloadProgress = {
  model: ModelEntry;
  downloadedBytes: number;
  totalBytes: number;
  percent: number;
};

export type DownloadOptions = {
  onProgress?: (progress: DownloadProgress) => void;
  signal?: AbortSignal;
};

export async function downloadModel(
  model: ModelEntry,
  destPath: string,
  options?: DownloadOptions,
): Promise<void> {
  // Skip if already exists and has reasonable size
  try {
    const s = await stat(destPath);
    if (s.size > 0) return;
  } catch {
    /* doesn't exist, proceed */
  }

  await mkdir(dirname(destPath), { recursive: true });

  const partPath = destPath + ".part";
  const response = await fetch(model.url, { signal: options?.signal });
  if (!response.ok) {
    throw new Error(`Download failed: ${response.status} ${response.statusText}`);
  }

  const totalBytes = Number(response.headers.get("content-length") ?? model.sizeBytes);
  let downloadedBytes = 0;

  const fileStream = createWriteStream(partPath);
  const reader = response.body!.getReader();

  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;

      fileStream.write(value);
      downloadedBytes += value.length;
      options?.onProgress?.({
        model,
        downloadedBytes,
        totalBytes,
        percent: Math.round((downloadedBytes / totalBytes) * 100),
      });
    }
    fileStream.end();
    await new Promise<void>((resolve, reject) => {
      fileStream.on("finish", resolve);
      fileStream.on("error", reject);
    });

    // Atomic rename
    await rename(partPath, destPath);
  } catch (err) {
    fileStream.destroy();
    await unlink(partPath).catch(() => {});
    throw err;
  }
}
