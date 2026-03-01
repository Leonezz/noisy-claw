import type { AudioEvent, SttConfig } from "../../ipc/protocol.js";
import type { AudioSubprocess } from "../../ipc/subprocess.js";
import type { AudioSource, AudioConfig, AudioChunk } from "../interfaces.js";

export class RustLocalCapture implements AudioSource {
  private audioCallbacks: Array<(chunk: AudioChunk) => void> = [];
  private vadCallbacks: Array<(speaking: boolean) => void> = [];
  private sttConfig: SttConfig | undefined;

  constructor(private readonly subprocess: AudioSubprocess) {}

  /** Set cloud STT config before calling start(). */
  setSttConfig(config: SttConfig | undefined): void {
    this.sttConfig = config;
  }

  start(config: AudioConfig): void {
    this.subprocess.send({
      cmd: "start_capture",
      device: config.device,
      sample_rate: config.sampleRate,
      stt: this.sttConfig,
    });
  }

  stop(): void {
    this.subprocess.trySend({ cmd: "stop_capture" });
  }

  onAudio(cb: (chunk: AudioChunk) => void): void {
    this.audioCallbacks.push(cb);
  }

  onVAD(cb: (speaking: boolean) => void): void {
    this.vadCallbacks.push(cb);
  }

  /** Called by the coordinator when IPC events arrive. */
  handleEvent(event: AudioEvent): void {
    if (event.event === "vad") {
      for (const cb of this.vadCallbacks) {
        cb(event.speaking);
      }
    }
  }
}
