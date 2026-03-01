import { spawn, type ChildProcess } from "node:child_process";
import { createInterface, type Interface } from "node:readline";
import {
  type Command,
  type AudioEvent,
  type SttConfig,
  type TtsConfig,
  parseEvent,
  serializeCommand,
} from "./protocol.js";

export type SubprocessOptions = {
  binaryPath: string;
  modelsDir?: string;
  sttModel?: string;
  onEvent: (event: AudioEvent) => void;
  onError: (error: Error) => void;
  onExit: (code: number | null) => void;
};

export class AudioSubprocess {
  private process: ChildProcess | null = null;
  private readline: Interface | null = null;
  private readonly options: SubprocessOptions;

  constructor(options: SubprocessOptions) {
    this.options = options;
  }

  start(): void {
    if (this.process) {
      throw new Error("Subprocess already running");
    }

    const env: Record<string, string> = {
      ...(process.env as Record<string, string>),
      RUST_LOG: "noisy_claw_audio=info",
    };
    if (this.options.modelsDir) {
      env.NOISY_CLAW_MODELS_DIR = this.options.modelsDir;
    }
    if (this.options.sttModel) {
      env.NOISY_CLAW_STT_MODEL = this.options.sttModel;
    }

    console.log(`[noisy-claw] spawning subprocess: ${this.options.binaryPath}`);
    console.log(`[noisy-claw] NOISY_CLAW_MODELS_DIR=${env.NOISY_CLAW_MODELS_DIR ?? "(unset)"}`);

    this.process = spawn(this.options.binaryPath, [], {
      stdio: ["pipe", "pipe", "pipe"],
      env,
    });

    console.log(`[noisy-claw] subprocess pid=${this.process.pid ?? "failed"}`);

    // Absorb EPIPE errors on stdin — the subprocess may exit before we finish writing
    this.process.stdin?.on("error", () => {});

    this.readline = createInterface({ input: this.process.stdout! });

    this.readline.on("line", (line) => {
      const event = parseEvent(line);
      if (event) {
        this.options.onEvent(event);
      }
    });

    this.process.stderr?.on("data", (data) => {
      const msg = data.toString().trim();
      if (msg) {
        console.log(`[noisy-claw-audio] ${msg}`);
      }
    });

    this.process.on("error", (err) => {
      this.options.onError(err);
    });

    this.process.on("exit", (code) => {
      this.process = null;
      this.readline = null;
      this.options.onExit(code);
    });
  }

  send(command: Command): void {
    if (!this.process?.stdin?.writable) {
      throw new Error("Subprocess not running or stdin not writable");
    }
    this.process.stdin.write(serializeCommand(command) + "\n");
  }

  /** Try to send a command, returning false if the subprocess is gone. */
  trySend(command: Command): boolean {
    try {
      if (!this.process?.stdin?.writable) {
        return false;
      }
      this.process.stdin.write(serializeCommand(command) + "\n");
      return true;
    } catch {
      return false;
    }
  }

  /** Send a speak command with TTS config. */
  speak(text: string, ttsConfig: TtsConfig): void {
    this.send({ cmd: "speak", text, tts: ttsConfig });
  }

  /** Stop current TTS speech playback. */
  stopSpeaking(): void {
    this.trySend({ cmd: "stop_speaking" });
  }

  /** Start a streaming TTS session. */
  speakStart(ttsConfig: TtsConfig): void {
    this.send({ cmd: "speak_start", tts: ttsConfig });
  }

  /** Send a text chunk to the active TTS session. */
  speakChunk(text: string): void {
    this.send({ cmd: "speak_chunk", text });
  }

  /** Signal end of text for the active TTS session. */
  speakEnd(): void {
    this.send({ cmd: "speak_end" });
  }

  /** Pause audio capture (stop_capture without shutting down). */
  pauseCapture(): void {
    this.trySend({ cmd: "stop_capture" });
  }

  /** Resume audio capture with the given config. */
  resumeCapture(config: {
    device?: string;
    sample_rate?: number;
    stt?: SttConfig;
  }): void {
    this.send({
      cmd: "start_capture",
      device: config.device,
      sample_rate: config.sample_rate,
      stt: config.stt,
    });
  }

  stop(): void {
    if (this.process) {
      this.trySend({ cmd: "shutdown" });
      const killTimer = setTimeout(() => {
        this.process?.kill("SIGKILL");
      }, 2000);
      this.process.on("exit", () => clearTimeout(killTimer));
    }
  }

  get isRunning(): boolean {
    return this.process !== null;
  }
}
