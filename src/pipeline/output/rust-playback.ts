import type { AudioEvent, TtsConfig } from "../../ipc/protocol.js";
import type { AudioSubprocess } from "../../ipc/subprocess.js";
import type { AudioOutput } from "../interfaces.js";

export class RustLocalPlayback implements AudioOutput {
  private playing = false;
  private doneCallbacks: Array<(requestId: string, reason: string) => void> = [];
  private activeRequestId: string | null = null;
  private ttsConfig: TtsConfig | null = null;

  constructor(private readonly subprocess: AudioSubprocess) {}

  setTtsConfig(config: TtsConfig): void {
    this.ttsConfig = config;
  }

  speak(text: string, requestId: string): void {
    if (!this.ttsConfig) throw new Error("No TTS config set");
    this.playing = true;
    this.activeRequestId = requestId;
    this.subprocess.trySend({
      cmd: "speak",
      text,
      tts: this.ttsConfig,
      request_id: requestId,
    });
  }

  speakStart(requestId: string): void {
    if (!this.ttsConfig) throw new Error("No TTS config set");
    this.playing = true;
    this.activeRequestId = requestId;
    this.subprocess.trySend({
      cmd: "speak_start",
      tts: this.ttsConfig,
      request_id: requestId,
    });
  }

  speakChunk(text: string, _requestId: string): void {
    this.subprocess.trySend({
      cmd: "speak_chunk",
      text,
    });
  }

  speakEnd(_requestId: string): void {
    this.subprocess.trySend({ cmd: "speak_end" });
  }

  stop(): void {
    this.subprocess.trySend({ cmd: "stop_speaking" });
    this.playing = false;
    this.activeRequestId = null;
  }

  flush(requestId: string): void {
    this.subprocess.trySend({ cmd: "flush_speak", request_id: requestId });
  }

  isPlaying(): boolean {
    return this.playing;
  }

  onDone(cb: (requestId: string, reason: string) => void): void {
    this.doneCallbacks.push(cb);
  }

  /** Called by the gateway when IPC events arrive. */
  handleEvent(event: AudioEvent): void {
    if (event.event === "speak_done") {
      this.playing = false;
      const reqId = event.request_id ?? this.activeRequestId ?? "";
      const reason = event.reason;
      this.activeRequestId = null;
      for (const cb of this.doneCallbacks) {
        cb(reqId, reason);
      }
    }
    if (event.event === "playback_done") {
      this.playing = false;
    }
  }
}
