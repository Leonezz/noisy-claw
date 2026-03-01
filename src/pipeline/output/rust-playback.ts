import type { AudioEvent, TtsConfig } from "../../ipc/protocol.js";
import type { AudioSubprocess } from "../../ipc/subprocess.js";
import type { AudioOutput } from "../interfaces.js";

export class RustLocalPlayback implements AudioOutput {
  private playing = false;
  private doneCallbacks: Array<() => void> = [];
  private playResolve: (() => void) | null = null;
  private speakEndResolve: (() => void) | null = null;
  private ttsConfig: TtsConfig | null = null;

  constructor(private readonly subprocess: AudioSubprocess) {}

  setTtsConfig(config: TtsConfig): void {
    this.ttsConfig = config;
  }

  speak(text: string): Promise<void> {
    if (!this.ttsConfig) {
      return Promise.reject(new Error("No TTS config set"));
    }
    return new Promise<void>((resolve) => {
      this.playing = true;
      this.playResolve = resolve;
      this.subprocess.speak(text, this.ttsConfig!);
    });
  }

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
    // stop_speaking handles both streaming TTS and file-based playback
    this.subprocess.trySend({ cmd: "stop_speaking" });
    this.playing = false;
    if (this.playResolve) {
      this.playResolve();
      this.playResolve = null;
    }
    if (this.speakEndResolve) {
      this.speakEndResolve();
      this.speakEndResolve = null;
    }
  }

  isPlaying(): boolean {
    return this.playing;
  }

  onDone(cb: () => void): void {
    this.doneCallbacks.push(cb);
  }

  speakStart(): void {
    if (!this.ttsConfig) {
      throw new Error("No TTS config set");
    }
    this.playing = true;
    this.subprocess.speakStart(this.ttsConfig);
  }

  speakChunk(text: string): void {
    this.subprocess.speakChunk(text);
  }

  speakEnd(): Promise<void> {
    return new Promise<void>((resolve) => {
      this.speakEndResolve = resolve;
      this.subprocess.speakEnd();
    });
  }

  /** Called by the coordinator when IPC events arrive. */
  handleEvent(event: AudioEvent): void {
    if (event.event === "playback_done" || event.event === "speak_done") {
      this.playing = false;
      if (this.playResolve) {
        this.playResolve();
        this.playResolve = null;
      }
      if (this.speakEndResolve) {
        this.speakEndResolve();
        this.speakEndResolve = null;
      }
      for (const cb of this.doneCallbacks) {
        cb();
      }
    }
  }
}
