import type { AudioEvent } from "../../ipc/protocol.js";
import type { AudioSubprocess } from "../../ipc/subprocess.js";
import type { AudioOutput } from "../interfaces.js";

export class RustLocalPlayback implements AudioOutput {
  private playing = false;
  private doneCallbacks: Array<() => void> = [];
  private playResolve: (() => void) | null = null;

  constructor(private readonly subprocess: AudioSubprocess) {}

  play(audioPath: string): Promise<void> {
    return new Promise<void>((resolve) => {
      this.playing = true;
      this.playResolve = resolve;
      if (!this.subprocess.trySend({ cmd: "play_audio", path: audioPath })) {
        this.playing = false;
        this.playResolve = null;
        resolve();
      }
    });
  }

  stop(): void {
    this.subprocess.trySend({ cmd: "stop_playback" });
    this.playing = false;
    if (this.playResolve) {
      this.playResolve();
      this.playResolve = null;
    }
  }

  isPlaying(): boolean {
    return this.playing;
  }

  onDone(cb: () => void): void {
    this.doneCallbacks.push(cb);
  }

  /** Called by the coordinator when IPC events arrive. */
  handleEvent(event: AudioEvent): void {
    if (event.event === "playback_done") {
      this.playing = false;
      if (this.playResolve) {
        this.playResolve();
        this.playResolve = null;
      }
      for (const cb of this.doneCallbacks) {
        cb();
      }
    }
  }
}
