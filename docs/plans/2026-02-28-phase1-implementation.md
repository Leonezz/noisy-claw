# Noisy-Claw Phase 1 MVP — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a working voice channel for OpenClaw — user speaks into mic, agent hears text, agent responds via TTS audio playback.

**Architecture:** OpenClaw extension plugin (TypeScript) + Rust audio subprocess. The TS plugin implements OpenClaw's `ChannelPlugin` interface and orchestrates a pluggable pipeline (AudioSource → STT → Segmentation → message, and agent response → TTS → AudioOutput). The Rust binary handles mic capture (cpal), VAD (silero ONNX), STT (whisper-rs), and audio playback (rodio), communicating with Node.js via JSON lines over stdin/stdout.

**Tech Stack:** TypeScript (OpenClaw plugin SDK, TypeBox schemas, Zod config validation), Rust (cpal, whisper-rs, rodio, ort/onnxruntime, serde, tokio), pnpm monorepo.

**Reference:** Design doc at `docs/plans/2026-02-28-phase1-mvp-design.md`. OpenClaw codebase at `/Users/zhuwenq/Documents/projects/openclaw/`. Key reference files:
- Plugin SDK: `openclaw/src/plugin-sdk/index.ts`
- Channel types: `openclaw/src/channels/plugins/types.plugin.ts`, `types.adapters.ts`
- Matrix extension (reference impl): `openclaw/extensions/matrix/`
- TTS: `openclaw/src/tts/tts.ts`
- Tool pattern: `openclaw/src/agents/tools/tts-tool.ts`

---

## Task 1: Project Scaffolding — TypeScript

**Files:**
- Create: `extensions/noisy-claw/package.json`
- Create: `extensions/noisy-claw/tsconfig.json`
- Create: `extensions/noisy-claw/src/index.ts` (stub)

**Step 1: Create directory structure**

```bash
mkdir -p extensions/noisy-claw/src/{channel,pipeline/{sources,stt,segmentation,tts,output},tools,ipc,config}
```

**Step 2: Create package.json**

Follow the Matrix extension pattern (`openclaw/extensions/matrix/package.json`). Key fields:

```json
{
  "name": "@openclaw/noisy-claw",
  "version": "0.1.0",
  "description": "OpenClaw voice channel plugin — bidirectional voice as a first-class channel",
  "type": "module",
  "dependencies": {
    "zod": "^4.3.6"
  },
  "openclaw": {
    "extensions": ["./index.ts"],
    "channel": {
      "id": "voice",
      "label": "Voice",
      "selectionLabel": "Voice (noisy-claw)",
      "docsPath": "/channels/voice",
      "docsLabel": "voice",
      "blurb": "bidirectional voice channel; speak to your agent, hear it respond.",
      "order": 80,
      "quickstartAllowFrom": false
    },
    "install": {
      "npmSpec": "@openclaw/noisy-claw",
      "localPath": "extensions/noisy-claw",
      "defaultChoice": "local"
    }
  }
}
```

**Step 3: Create tsconfig.json**

Extensions use the root tsconfig, but create a local one that extends it:

```json
{
  "extends": "../../tsconfig.json",
  "include": ["src/**/*"]
}
```

**Step 4: Create stub index.ts**

```typescript
import type { OpenClawPluginApi } from "openclaw/plugin-sdk";
import { emptyPluginConfigSchema } from "openclaw/plugin-sdk";

const plugin = {
  id: "noisy-claw",
  name: "Noisy Claw",
  description: "Voice channel plugin — bidirectional voice as a first-class channel",
  configSchema: emptyPluginConfigSchema(),
  register(api: OpenClawPluginApi) {
    // TODO: register channel and tools
    console.log("[noisy-claw] plugin registered");
  },
};

export default plugin;
```

**Step 5: Verify TypeScript compiles**

```bash
cd /path/to/openclaw && pnpm exec tsc --noEmit extensions/noisy-claw/src/index.ts
```

Expected: no errors (or only path resolution warnings since we're outside the monorepo initially).

**Step 6: Commit**

```bash
git add extensions/noisy-claw/
git commit -m "feat: scaffold noisy-claw extension with package.json and stub entry"
```

---

## Task 2: Project Scaffolding — Rust

**Files:**
- Create: `extensions/noisy-claw/native/noisy-claw-audio/Cargo.toml`
- Create: `extensions/noisy-claw/native/noisy-claw-audio/src/main.rs` (stub)
- Create: `extensions/noisy-claw/native/noisy-claw-audio/src/protocol.rs`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "noisy-claw-audio"
version = "0.1.0"
edition = "2021"
description = "Audio engine for noisy-claw: capture, VAD, STT, playback"

[dependencies]
# Audio capture & playback
cpal = "0.15"
rodio = { version = "0.20", default-features = false, features = ["mp3", "vorbis", "wav"] }

# STT
whisper-rs = "0.13"

# VAD (ONNX inference)
ort = { version = "2.0", features = ["download-binaries"] }
ndarray = "0.16"

# IPC
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Async runtime (needed now for clean concurrency, and later for cloud STT/TTS providers)
tokio = { version = "1", features = ["full"] }

# Utilities
anyhow = "1"
base64 = "0.22"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

**Step 2: Create stub main.rs**

```rust
mod protocol;

use anyhow::Result;
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("noisy_claw_audio=info")
        .init();

    tracing::info!("noisy-claw-audio starting");

    // Emit ready event
    {
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        let event = protocol::Event::Ready;
        serde_json::to_writer(&mut stdout, &event)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            continue;
        }

        match serde_json::from_str::<protocol::Command>(&line) {
            Ok(cmd) => {
                tracing::info!(?cmd, "received command");
                // TODO: dispatch commands
                let stdout = std::io::stdout();
                let mut stdout = stdout.lock();
                let event = protocol::Event::Error {
                    message: "not implemented".to_string(),
                };
                serde_json::to_writer(&mut stdout, &event)?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
            }
            Err(e) => {
                tracing::error!(%e, "failed to parse command");
            }
        }
    }

    Ok(())
}
```

**Step 3: Create protocol.rs**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    StartCapture {
        #[serde(default = "default_device")]
        device: String,
        #[serde(default = "default_sample_rate")]
        sample_rate: u32,
    },
    StopCapture,
    PlayAudio {
        path: String,
    },
    StopPlayback,
    GetStatus,
    Shutdown,
}

fn default_device() -> String {
    "default".to_string()
}

fn default_sample_rate() -> u32 {
    16000
}

#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Ready,
    Vad {
        speaking: bool,
    },
    Transcript {
        text: String,
        is_final: bool,
        start: f64,
        end: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        confidence: Option<f64>,
    },
    PlaybackDone,
    Status {
        capturing: bool,
        playing: bool,
    },
    Error {
        message: String,
    },
}
```

**Step 4: Verify Rust compiles**

```bash
cd extensions/noisy-claw/native/noisy-claw-audio && cargo check
```

Expected: compiles (dependencies download, no errors). Note: `whisper-rs` and `ort` may take time to download/compile native libraries.

**Step 5: Run cargo test (empty for now)**

```bash
cargo test
```

Expected: 0 tests, pass.

**Step 6: Commit**

```bash
git add extensions/noisy-claw/native/
git commit -m "feat: scaffold Rust audio engine with IPC protocol types"
```

---

## Task 3: IPC Protocol — TypeScript Side

**Files:**
- Create: `extensions/noisy-claw/src/ipc/protocol.ts`
- Create: `extensions/noisy-claw/src/ipc/subprocess.ts`

**Step 1: Create TypeScript IPC protocol types**

These must mirror the Rust `protocol.rs` exactly.

```typescript
// src/ipc/protocol.ts

// Commands sent from Node.js to Rust (via stdin)
export type StartCaptureCommand = {
  cmd: "start_capture";
  device?: string;    // default: "default"
  sample_rate?: number; // default: 16000
};

export type StopCaptureCommand = {
  cmd: "stop_capture";
};

export type PlayAudioCommand = {
  cmd: "play_audio";
  path: string;
};

export type StopPlaybackCommand = {
  cmd: "stop_playback";
};

export type GetStatusCommand = {
  cmd: "get_status";
};

export type ShutdownCommand = {
  cmd: "shutdown";
};

export type Command =
  | StartCaptureCommand
  | StopCaptureCommand
  | PlayAudioCommand
  | StopPlaybackCommand
  | GetStatusCommand
  | ShutdownCommand;

// Events received from Rust (via stdout)
export type ReadyEvent = {
  event: "ready";
};

export type VadEvent = {
  event: "vad";
  speaking: boolean;
};

export type TranscriptEvent = {
  event: "transcript";
  text: string;
  is_final: boolean;
  start: number;
  end: number;
  confidence?: number;
};

export type PlaybackDoneEvent = {
  event: "playback_done";
};

export type StatusEvent = {
  event: "status";
  capturing: boolean;
  playing: boolean;
};

export type ErrorEvent = {
  event: "error";
  message: string;
};

export type AudioEvent =
  | ReadyEvent
  | VadEvent
  | TranscriptEvent
  | PlaybackDoneEvent
  | StatusEvent
  | ErrorEvent;

export function parseEvent(line: string): AudioEvent | null {
  try {
    return JSON.parse(line) as AudioEvent;
  } catch {
    return null;
  }
}

export function serializeCommand(cmd: Command): string {
  return JSON.stringify(cmd);
}
```

**Step 2: Create subprocess manager**

```typescript
// src/ipc/subprocess.ts

import { spawn, type ChildProcess } from "node:child_process";
import { createInterface, type Interface } from "node:readline";
import { EventEmitter } from "node:events";
import { type Command, type AudioEvent, parseEvent, serializeCommand } from "./protocol.js";

export type SubprocessOptions = {
  binaryPath: string;
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

    this.process = spawn(this.options.binaryPath, [], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, RUST_LOG: "noisy_claw_audio=info" },
    });

    this.readline = createInterface({ input: this.process.stdout! });

    this.readline.on("line", (line) => {
      const event = parseEvent(line);
      if (event) {
        this.options.onEvent(event);
      }
    });

    this.process.stderr?.on("data", (data) => {
      // Rust tracing output goes to stderr — log it
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

  stop(): void {
    if (this.process) {
      this.send({ cmd: "shutdown" });
      // Give it 2 seconds to exit gracefully, then kill
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
```

**Step 3: Commit**

```bash
git add extensions/noisy-claw/src/ipc/
git commit -m "feat: add IPC protocol types and subprocess manager"
```

---

## Task 4: Pipeline Interfaces

**Files:**
- Create: `extensions/noisy-claw/src/pipeline/interfaces.ts`

**Step 1: Define all pluggable pipeline interfaces**

```typescript
// src/pipeline/interfaces.ts

// --- Shared Types ---

export type AudioConfig = {
  device: string;
  sampleRate: number;
};

export type AudioChunk = {
  data: Buffer;       // Raw PCM samples (16-bit signed, mono)
  timestamp: number;  // Seconds from stream start
};

export type STTConfig = {
  model: string;      // e.g. "base", "small", "medium"
  language: string;   // e.g. "en", "auto"
};

export type TranscriptSegment = {
  text: string;
  isFinal: boolean;
  start: number;      // Seconds
  end: number;        // Seconds
  confidence?: number;
};

export type SegmentMetadata = {
  startTime: number;
  endTime: number;
  segmentCount: number;
};

export type TTSOpts = {
  voice?: string;
  speed?: number;
};

// --- Pipeline Interfaces ---

export interface AudioSource {
  start(config: AudioConfig): void;
  stop(): void;
  onAudio(cb: (chunk: AudioChunk) => void): void;
  onVAD(cb: (speaking: boolean) => void): void;
}

export interface STTProvider {
  start(config: STTConfig): void;
  stop(): void;
  feed(chunk: AudioChunk): void;
  onTranscript(cb: (segment: TranscriptSegment) => void): void;
}

export interface SegmentationEngine {
  onTranscript(segment: TranscriptSegment): void;
  onVAD(speaking: boolean): void;
  onMessage(cb: (message: string, metadata: SegmentMetadata) => void): void;
  flush(): string | null;
}

export interface TTSProvider {
  synthesize(text: string, opts?: TTSOpts): Promise<string>; // returns audio file path
}

export interface AudioOutput {
  play(audioPath: string): Promise<void>;
  stop(): void;
  isPlaying(): boolean;
  onDone(cb: () => void): void;
}
```

**Step 2: Commit**

```bash
git add extensions/noisy-claw/src/pipeline/interfaces.ts
git commit -m "feat: define pluggable pipeline interfaces"
```

---

## Task 5: Rust Audio Capture Module

**Files:**
- Create: `extensions/noisy-claw/native/noisy-claw-audio/src/capture.rs`
- Modify: `extensions/noisy-claw/native/noisy-claw-audio/src/main.rs`

**Step 1: Implement capture.rs**

Uses `cpal` to capture from the default input device. Bridges cpal's synchronous callback into tokio via `tokio::sync::mpsc::UnboundedSender` (unbounded because the audio callback is real-time and must never block).

```rust
// src/capture.rs

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

pub struct AudioCapture {
    stream: Option<Stream>,
    running: Arc<AtomicBool>,
}

pub type AudioFrame = Vec<f32>; // Mono f32 samples at target sample rate

impl AudioCapture {
    pub fn new() -> Self {
        Self {
            stream: None,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start capturing from the default input device.
    /// Returns a tokio receiver that yields audio frames.
    /// Uses an unbounded channel because cpal's audio callback is real-time
    /// and must never block.
    pub fn start(
        &mut self,
        device_name: &str,
        sample_rate: u32,
    ) -> Result<mpsc::UnboundedReceiver<AudioFrame>> {
        let host = cpal::default_host();

        let device = if device_name == "default" {
            host.default_input_device()
                .ok_or_else(|| anyhow!("no default input device"))?
        } else {
            host.input_devices()?
                .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
                .ok_or_else(|| anyhow!("input device '{}' not found", device_name))?
        };

        tracing::info!(device = %device.name().unwrap_or_default(), "using input device");

        let config = StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let (tx, rx) = mpsc::unbounded_channel::<AudioFrame>();
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();

        let stream = device.build_input_stream(
            &config,
            move |data: &[f32], _info| {
                if running.load(Ordering::SeqCst) {
                    // send() on UnboundedSender never blocks — safe in audio callback
                    let _ = tx.send(data.to_vec());
                }
            },
            |err| {
                tracing::error!(%err, "audio capture error");
            },
            None,
        )?;

        stream.play()?;
        self.stream = Some(stream);

        Ok(rx)
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.stream = None;
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// List available input devices.
pub fn list_input_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let devices: Vec<String> = host
        .input_devices()?
        .filter_map(|d| d.name().ok())
        .collect();
    Ok(devices)
}
```

**Step 2: Add `mod capture;` to main.rs**

Add `mod capture;` at the top of main.rs alongside `mod protocol;`.

**Step 3: Verify compiles**

```bash
cd extensions/noisy-claw/native/noisy-claw-audio && cargo check
```

**Step 4: Commit**

```bash
git add extensions/noisy-claw/native/noisy-claw-audio/src/capture.rs
git commit -m "feat: implement audio capture module with cpal + tokio bridge"
```

---

## Task 6: Rust VAD Module

**Files:**
- Create: `extensions/noisy-claw/native/noisy-claw-audio/src/vad.rs`
- Create: `extensions/noisy-claw/native/noisy-claw-audio/build.rs` (optional, for model download)

**Step 1: Implement VAD using Silero ONNX model**

The Silero VAD model processes 512-sample windows at 16kHz and outputs a speech probability.

```rust
// src/vad.rs

use anyhow::Result;
use ndarray::{Array1, Array2, Array3};
use ort::session::Session;
use std::path::Path;

const WINDOW_SIZE: usize = 512; // 32ms at 16kHz
const SAMPLE_RATE: i64 = 16000;

pub struct VoiceActivityDetector {
    session: Session,
    // Hidden state tensors (Silero VAD is stateful)
    h: Array3<f32>,
    c: Array3<f32>,
    threshold: f32,
    triggered: bool,
    // Buffer for accumulating samples until we have a full window
    buffer: Vec<f32>,
}

impl VoiceActivityDetector {
    pub fn new(model_path: &Path, threshold: f32) -> Result<Self> {
        let session = Session::builder()?
            .with_intra_threads(1)?
            .commit_from_file(model_path)?;

        Ok(Self {
            session,
            h: Array3::zeros((2, 1, 64)),
            c: Array3::zeros((2, 1, 64)),
            threshold,
            triggered: false,
            buffer: Vec::with_capacity(WINDOW_SIZE),
        })
    }

    /// Process audio samples and return VAD state changes.
    /// Returns Some(true) on speech start, Some(false) on speech end, None if unchanged.
    pub fn process(&mut self, samples: &[f32]) -> Vec<bool> {
        let mut transitions = Vec::new();
        self.buffer.extend_from_slice(samples);

        while self.buffer.len() >= WINDOW_SIZE {
            let window: Vec<f32> = self.buffer.drain(..WINDOW_SIZE).collect();
            let input = Array2::from_shape_vec((1, WINDOW_SIZE), window)
                .expect("shape mismatch");
            let sr = Array1::from_vec(vec![SAMPLE_RATE]);

            let outputs = self.session.run(
                ort::inputs![
                    "input" => input.view(),
                    "sr" => sr.view(),
                    "h" => self.h.view(),
                    "c" => self.c.view(),
                ]
                .expect("input construction failed"),
            ).expect("VAD inference failed");

            let prob = outputs["output"]
                .try_extract_tensor::<f32>()
                .expect("output extraction failed");
            let speech_prob = prob.as_slice().unwrap()[0];

            // Update hidden states
            self.h = outputs["hn"]
                .try_extract_tensor::<f32>()
                .expect("hn extraction failed")
                .to_owned()
                .into_dimensionality()
                .expect("hn shape mismatch");
            self.c = outputs["cn"]
                .try_extract_tensor::<f32>()
                .expect("cn extraction failed")
                .to_owned()
                .into_dimensionality()
                .expect("cn shape mismatch");

            let is_speech = speech_prob >= self.threshold;
            if is_speech != self.triggered {
                self.triggered = is_speech;
                transitions.push(is_speech);
            }
        }

        transitions
    }

    pub fn is_speaking(&self) -> bool {
        self.triggered
    }

    pub fn reset(&mut self) {
        self.h.fill(0.0);
        self.c.fill(0.0);
        self.triggered = false;
        self.buffer.clear();
    }
}
```

**Step 2: Add `mod vad;` to main.rs**

**Step 3: Verify compiles**

```bash
cargo check
```

**Step 4: Commit**

```bash
git add extensions/noisy-claw/native/noisy-claw-audio/src/vad.rs
git commit -m "feat: implement Silero VAD module with ONNX runtime"
```

---

## Task 7: Rust STT Module

**Files:**
- Create: `extensions/noisy-claw/native/noisy-claw-audio/src/stt.rs`

**Step 1: Implement whisper-rs STT**

```rust
// src/stt.rs

use anyhow::Result;
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct TranscriptSegment {
    pub text: String,
    pub start: f64,  // seconds
    pub end: f64,    // seconds
    pub is_final: bool,
}

pub struct WhisperSTT {
    ctx: WhisperContext,
    language: String,
}

impl WhisperSTT {
    pub fn new(model_path: &Path, language: &str) -> Result<Self> {
        let ctx = WhisperContext::new_with_params(
            model_path.to_str().unwrap(),
            WhisperContextParameters::default(),
        )?;

        Ok(Self {
            ctx,
            language: language.to_string(),
        })
    }

    /// Transcribe a buffer of audio samples (f32, mono, 16kHz).
    /// Returns transcript segments.
    pub fn transcribe(&self, samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        if self.language != "auto" {
            params.set_language(Some(&self.language));
        }

        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_single_segment(false);

        let mut state = self.ctx.create_state()?;
        state.full(params, samples)?;

        let n_segments = state.full_n_segments()?;
        let mut segments = Vec::with_capacity(n_segments as usize);

        for i in 0..n_segments {
            let text = state.full_get_segment_text(i)?;
            let start = state.full_get_segment_t0(i)? as f64 / 100.0; // centiseconds to seconds
            let end = state.full_get_segment_t1(i)? as f64 / 100.0;

            if !text.trim().is_empty() {
                segments.push(TranscriptSegment {
                    text: text.trim().to_string(),
                    start,
                    end,
                    is_final: true,
                });
            }
        }

        Ok(segments)
    }
}
```

**Step 2: Add `mod stt;` to main.rs**

**Step 3: Verify compiles**

```bash
cargo check
```

**Step 4: Commit**

```bash
git add extensions/noisy-claw/native/noisy-claw-audio/src/stt.rs
git commit -m "feat: implement whisper-rs STT module"
```

---

## Task 8: Rust Audio Playback Module

**Files:**
- Create: `extensions/noisy-claw/native/noisy-claw-audio/src/playback.rs`

**Step 1: Implement playback.rs**

Uses `rodio` for audio output. Signals completion via a `tokio::sync::mpsc` sender rather than polling. The `Sink::sleep_until_end()` call is blocking, so it runs in a `tokio::task::spawn_blocking` wrapper (handled by the caller in main.rs).

```rust
// src/playback.rs

use anyhow::Result;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct AudioPlayback {
    // OutputStream must be kept alive for the duration of playback
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    sink: Option<Arc<Sink>>,
    playing: Arc<AtomicBool>,
}

impl AudioPlayback {
    pub fn new() -> Result<Self> {
        let (stream, handle) = OutputStream::try_default()?;
        Ok(Self {
            _stream: Some(stream),
            handle: Some(handle),
            sink: None,
            playing: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Play an audio file. Returns the Sink wrapped in Arc so the caller
    /// can wait for completion via `spawn_blocking(|| sink.sleep_until_end())`.
    pub fn play(&mut self, path: &Path) -> Result<Arc<Sink>> {
        // Stop any current playback
        self.stop();

        let handle = self.handle.as_ref()
            .ok_or_else(|| anyhow::anyhow!("no output stream"))?;

        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let source = Decoder::new(reader)?;

        let sink = Arc::new(Sink::try_new(handle)?);
        sink.append(source);
        self.playing.store(true, Ordering::SeqCst);
        self.sink = Some(sink.clone());

        Ok(sink)
    }

    pub fn stop(&mut self) {
        if let Some(ref sink) = self.sink.take() {
            sink.stop();
        }
        self.playing.store(false, Ordering::SeqCst);
    }

    pub fn is_playing(&self) -> bool {
        if let Some(ref sink) = self.sink {
            if sink.empty() {
                return false;
            }
        }
        self.playing.load(Ordering::SeqCst)
    }

    pub fn set_done(&self) {
        self.playing.store(false, Ordering::SeqCst);
    }
}
```

**Step 2: Add `mod playback;` to main.rs**

**Step 3: Verify compiles**

```bash
cargo check
```

**Step 4: Commit**

```bash
git add extensions/noisy-claw/native/noisy-claw-audio/src/playback.rs
git commit -m "feat: implement audio playback module with rodio"
```

---

## Task 9: Rust IPC Main Loop (tokio)

**Files:**
- Modify: `extensions/noisy-claw/native/noisy-claw-audio/src/main.rs`

This is the core wiring. Uses `#[tokio::main]` with `tokio::select!` to multiplex:
- Stdin command reading (async via `tokio::io::AsyncBufReadExt`)
- Audio frame processing (from cpal via `tokio::sync::mpsc`)
- Playback completion monitoring (via `spawn_blocking`)
- Event emission to stdout (via `tokio::sync::mpsc`)

No `Arc<Mutex<>>` needed — all mutable state lives in the main async task, and CPU-bound work (STT) is offloaded via `spawn_blocking`.

**Step 1: Implement the full main loop**

```rust
// src/main.rs

mod capture;
mod playback;
mod protocol;
mod stt;
mod vad;

use anyhow::Result;
use protocol::{Command, Event};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "noisy_claw_audio=info".to_string()),
        )
        .init();

    // Channel for events -> stdout writer task
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(256);

    // Spawn stdout writer task
    tokio::spawn(async move {
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        while let Some(event) = event_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event) {
                let _ = writeln!(stdout, "{}", json);
                let _ = stdout.flush();
            }
        }
    });

    // Mutable state — lives entirely in the main task, no Arc<Mutex<>> needed
    let mut capture = capture::AudioCapture::new();
    let mut playback = playback::AudioPlayback::new()?;
    let mut vad_engine: Option<vad::VoiceActivityDetector> = None;
    let mut stt_engine: Option<Arc<stt::WhisperSTT>> = None;
    let mut speech_buffer: Vec<f32> = Vec::new();
    let mut speech_start_time: Option<Instant> = None;
    let mut capture_start_time: Option<Instant> = None;
    let mut suppress_stt = false;
    let mut was_speaking = false;

    // Audio frame receiver — set when capture starts
    let mut audio_rx: Option<tokio::sync::mpsc::UnboundedReceiver<capture::AudioFrame>> = None;

    // Playback completion receiver — set when playback starts
    let mut playback_done_rx: Option<tokio::sync::oneshot::Receiver<()>> = None;

    // Send ready event
    event_tx.send(Event::Ready).await?;

    // Async stdin reader
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    loop {
        tokio::select! {
            // Branch 1: Read commands from stdin
            line = lines.next_line() => {
                let line = match line? {
                    Some(l) => l,
                    None => break, // EOF — stdin closed
                };
                if line.is_empty() {
                    continue;
                }

                match serde_json::from_str::<Command>(&line) {
                    Ok(cmd) => match cmd {
                        Command::StartCapture { device, sample_rate } => {
                            let models_dir = resolve_models_dir();

                            // Init VAD if needed
                            if vad_engine.is_none() {
                                let vad_path = models_dir.join("silero_vad.onnx");
                                match vad::VoiceActivityDetector::new(&vad_path, 0.5) {
                                    Ok(v) => vad_engine = Some(v),
                                    Err(e) => {
                                        let _ = event_tx.send(Event::Error {
                                            message: format!("VAD init failed: {e}"),
                                        }).await;
                                        continue;
                                    }
                                }
                            }

                            // Init STT if needed (wrapped in Arc for spawn_blocking)
                            if stt_engine.is_none() {
                                let model_path = models_dir.join("ggml-base.bin");
                                match stt::WhisperSTT::new(&model_path, "en") {
                                    Ok(s) => stt_engine = Some(Arc::new(s)),
                                    Err(e) => {
                                        let _ = event_tx.send(Event::Error {
                                            message: format!("STT init failed: {e}"),
                                        }).await;
                                        continue;
                                    }
                                }
                            }

                            // Start capture
                            match capture.start(&device, sample_rate) {
                                Ok(rx) => {
                                    audio_rx = Some(rx);
                                    capture_start_time = Some(Instant::now());
                                    tracing::info!("capture started");
                                }
                                Err(e) => {
                                    let _ = event_tx.send(Event::Error {
                                        message: format!("capture start failed: {e}"),
                                    }).await;
                                }
                            }
                        }

                        Command::StopCapture => {
                            capture.stop();
                            audio_rx = None;
                            // Flush remaining speech buffer
                            if !speech_buffer.is_empty() {
                                if let Some(ref stt) = stt_engine {
                                    let samples = std::mem::take(&mut speech_buffer);
                                    let stt = stt.clone();
                                    let tx = event_tx.clone();
                                    let base = compute_base_time(speech_start_time.take(), capture_start_time);
                                    tokio::task::spawn_blocking(move || {
                                        transcribe_and_emit(&stt, &samples, base, &tx);
                                    });
                                }
                            }
                            tracing::info!("capture stopped");
                        }

                        Command::PlayAudio { path } => {
                            suppress_stt = true;
                            match playback.play(std::path::Path::new(&path)) {
                                Ok(sink) => {
                                    tracing::info!(%path, "playback started");
                                    // Wait for playback completion in a blocking task
                                    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
                                    playback_done_rx = Some(done_rx);
                                    tokio::task::spawn_blocking(move || {
                                        sink.sleep_until_end();
                                        let _ = done_tx.send(());
                                    });
                                }
                                Err(e) => {
                                    suppress_stt = false;
                                    let _ = event_tx.send(Event::Error {
                                        message: format!("playback failed: {e}"),
                                    }).await;
                                }
                            }
                        }

                        Command::StopPlayback => {
                            playback.stop();
                            suppress_stt = false;
                            playback_done_rx = None;
                            tracing::info!("playback stopped");
                        }

                        Command::GetStatus => {
                            let _ = event_tx.send(Event::Status {
                                capturing: capture.is_running(),
                                playing: playback.is_playing(),
                            }).await;
                        }

                        Command::Shutdown => {
                            capture.stop();
                            playback.stop();
                            tracing::info!("shutting down");
                            break;
                        }
                    },
                    Err(e) => {
                        tracing::error!(%e, %line, "failed to parse command");
                        let _ = event_tx.send(Event::Error {
                            message: format!("invalid command: {e}"),
                        }).await;
                    }
                }
            }

            // Branch 2: Process audio frames from capture
            Some(frame) = async {
                match audio_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                // Always run VAD (even during playback, for interruption detection)
                if let Some(ref mut vad) = vad_engine {
                    let transitions = vad.process(&frame);
                    for speaking in transitions {
                        let _ = event_tx.send(Event::Vad { speaking }).await;

                        if speaking && !was_speaking {
                            speech_start_time = Some(Instant::now());
                        }

                        if !speaking && was_speaking && !suppress_stt {
                            // Speech ended — offload transcription to blocking task
                            if let Some(ref stt) = stt_engine {
                                let samples = std::mem::take(&mut speech_buffer);
                                let stt = stt.clone();
                                let tx = event_tx.clone();
                                let base = compute_base_time(speech_start_time.take(), capture_start_time);
                                tokio::task::spawn_blocking(move || {
                                    transcribe_and_emit(&stt, &samples, base, &tx);
                                });
                            }
                        }

                        was_speaking = speaking;
                    }
                }

                // Accumulate audio for STT (only when not suppressed)
                if !suppress_stt && was_speaking {
                    speech_buffer.extend_from_slice(&frame);
                }
            }

            // Branch 3: Playback completion
            Ok(()) = async {
                match playback_done_rx.as_mut() {
                    Some(rx) => rx.await.map_err(|_| ()),
                    None => std::future::pending::<std::result::Result<(), ()>>().await,
                }
            } => {
                suppress_stt = false;
                playback.set_done();
                playback_done_rx = None;
                let _ = event_tx.send(Event::PlaybackDone).await;
            }
        }
    }

    Ok(())
}

/// Compute the base timestamp for transcript segments.
fn compute_base_time(speech_start: Option<Instant>, capture_start: Option<Instant>) -> f64 {
    match (speech_start, capture_start) {
        (Some(st), Some(ct)) => st.duration_since(ct).as_secs_f64(),
        _ => 0.0,
    }
}

/// Run STT and send transcript events. Called from spawn_blocking.
fn transcribe_and_emit(
    stt: &stt::WhisperSTT,
    samples: &[f32],
    base_time: f64,
    event_tx: &mpsc::Sender<Event>,
) {
    match stt.transcribe(samples) {
        Ok(segments) => {
            for seg in segments {
                // Use blocking_send since we're in a spawn_blocking context
                let _ = event_tx.blocking_send(Event::Transcript {
                    text: seg.text,
                    is_final: seg.is_final,
                    start: base_time + seg.start,
                    end: base_time + seg.end,
                    confidence: None,
                });
            }
        }
        Err(e) => {
            tracing::error!(%e, "STT transcription failed");
            let _ = event_tx.blocking_send(Event::Error {
                message: format!("STT failed: {e}"),
            });
        }
    }
}

fn resolve_models_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));

    if let Some(ref dir) = exe_dir {
        let models = dir.join("models");
        if models.exists() {
            return models;
        }
    }

    PathBuf::from("models")
}
```

**Key tokio patterns used:**
- `#[tokio::main]` — async entry point
- `tokio::select!` — multiplexes stdin, audio frames, and playback completion in a single loop without threads or mutexes
- `tokio::task::spawn_blocking` — offloads CPU-bound whisper transcription and rodio's `sleep_until_end()` to the blocking thread pool
- `tokio::sync::mpsc` — async channels for events and audio frames
- `tokio::sync::oneshot` — single-use signal for playback completion
- `mpsc::Sender::blocking_send()` — sends from blocking context (spawn_blocking) into async channel
- No `Arc<Mutex<>>` on the engine — all mutable state stays in the main task

**Step 2: Build the full binary**

```bash
cd extensions/noisy-claw/native/noisy-claw-audio && cargo build
```

Expected: compiles successfully. Note: first build will be slow due to whisper-rs and ort downloading native libraries.

**Step 3: Commit**

```bash
git add extensions/noisy-claw/native/noisy-claw-audio/src/main.rs
git commit -m "feat: implement async IPC main loop with tokio::select!"
```

---

## Task 10: TypeScript Pipeline Implementations

**Files:**
- Create: `extensions/noisy-claw/src/pipeline/sources/rust-capture.ts`
- Create: `extensions/noisy-claw/src/pipeline/stt/rust-whisper.ts`
- Create: `extensions/noisy-claw/src/pipeline/output/rust-playback.ts`
- Create: `extensions/noisy-claw/src/pipeline/tts/openclaw-tts.ts`

These wrap the Rust subprocess IPC events into the pluggable pipeline interfaces.

**Step 1: RustLocalCapture (AudioSource)**

```typescript
// src/pipeline/sources/rust-capture.ts

import type { AudioSource, AudioConfig, AudioChunk } from "../interfaces.js";
import type { AudioSubprocess } from "../../ipc/subprocess.js";

export class RustLocalCapture implements AudioSource {
  private audioCallbacks: Array<(chunk: AudioChunk) => void> = [];
  private vadCallbacks: Array<(speaking: boolean) => void> = [];

  constructor(private readonly subprocess: AudioSubprocess) {}

  start(config: AudioConfig): void {
    this.subprocess.send({
      cmd: "start_capture",
      device: config.device,
      sample_rate: config.sampleRate,
    });
  }

  stop(): void {
    this.subprocess.send({ cmd: "stop_capture" });
  }

  onAudio(cb: (chunk: AudioChunk) => void): void {
    this.audioCallbacks.push(cb);
  }

  onVAD(cb: (speaking: boolean) => void): void {
    this.vadCallbacks.push(cb);
  }

  // Called by the coordinator when IPC events arrive
  handleEvent(event: { event: string; [key: string]: unknown }): void {
    if (event.event === "vad") {
      const speaking = event.speaking as boolean;
      for (const cb of this.vadCallbacks) {
        cb(speaking);
      }
    }
  }
}
```

**Step 2: RustWhisperSTT (STTProvider)**

```typescript
// src/pipeline/stt/rust-whisper.ts

import type { STTProvider, STTConfig, AudioChunk, TranscriptSegment } from "../interfaces.js";
import type { AudioSubprocess } from "../../ipc/subprocess.js";

export class RustWhisperSTT implements STTProvider {
  private transcriptCallbacks: Array<(segment: TranscriptSegment) => void> = [];

  constructor(private readonly subprocess: AudioSubprocess) {}

  start(_config: STTConfig): void {
    // STT is started implicitly when capture starts in the Rust process.
    // The config (model, language) is set at subprocess spawn time.
  }

  stop(): void {
    // STT stops when capture stops.
  }

  feed(_chunk: AudioChunk): void {
    // In the Rust subprocess architecture, audio flows directly from
    // capture -> VAD -> STT internally. We don't feed chunks from TS.
    // This method exists for cloud STT providers that receive chunks from TS.
  }

  onTranscript(cb: (segment: TranscriptSegment) => void): void {
    this.transcriptCallbacks.push(cb);
  }

  // Called by the coordinator when IPC events arrive
  handleEvent(event: { event: string; [key: string]: unknown }): void {
    if (event.event === "transcript") {
      const segment: TranscriptSegment = {
        text: event.text as string,
        isFinal: event.is_final as boolean,
        start: event.start as number,
        end: event.end as number,
        confidence: event.confidence as number | undefined,
      };
      for (const cb of this.transcriptCallbacks) {
        cb(segment);
      }
    }
  }
}
```

**Step 3: RustLocalPlayback (AudioOutput)**

```typescript
// src/pipeline/output/rust-playback.ts

import type { AudioOutput } from "../interfaces.js";
import type { AudioSubprocess } from "../../ipc/subprocess.js";

export class RustLocalPlayback implements AudioOutput {
  private playing = false;
  private doneCallbacks: Array<() => void> = [];
  private playResolve: (() => void) | null = null;

  constructor(private readonly subprocess: AudioSubprocess) {}

  play(audioPath: string): Promise<void> {
    return new Promise<void>((resolve) => {
      this.playing = true;
      this.playResolve = resolve;
      this.subprocess.send({ cmd: "play_audio", path: audioPath });
    });
  }

  stop(): void {
    this.subprocess.send({ cmd: "stop_playback" });
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

  // Called by the coordinator when IPC events arrive
  handleEvent(event: { event: string }): void {
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
```

**Step 4: OpenClawTTS (TTSProvider)**

```typescript
// src/pipeline/tts/openclaw-tts.ts

import type { TTSProvider, TTSOpts } from "../interfaces.js";

// This wraps OpenClaw's existing textToSpeech function.
// The actual import path depends on how the plugin accesses the core TTS module.
// We accept the TTS function as a dependency injection parameter.

export type TextToSpeechFn = (params: {
  text: string;
  cfg: unknown;
  channel?: string;
}) => Promise<{ success: boolean; audioPath?: string; error?: string }>;

export class OpenClawTTS implements TTSProvider {
  constructor(
    private readonly ttsFunction: TextToSpeechFn,
    private readonly config: unknown,
  ) {}

  async synthesize(text: string, _opts?: TTSOpts): Promise<string> {
    const result = await this.ttsFunction({
      text,
      cfg: this.config,
      channel: "voice",
    });

    if (!result.success || !result.audioPath) {
      throw new Error(result.error ?? "TTS synthesis failed");
    }

    return result.audioPath;
  }
}
```

**Step 5: Commit**

```bash
git add extensions/noisy-claw/src/pipeline/
git commit -m "feat: implement pipeline providers wrapping Rust subprocess and OpenClaw TTS"
```

---

## Task 11: Segmentation Engine — VAD Silence

**Files:**
- Create: `extensions/noisy-claw/src/pipeline/segmentation/vad-silence.ts`

**Step 1: Implement VADSilenceSegmentation**

```typescript
// src/pipeline/segmentation/vad-silence.ts

import type { SegmentationEngine, TranscriptSegment, SegmentMetadata } from "../interfaces.js";

export type VADSilenceConfig = {
  silenceThresholdMs: number;  // default 700ms
};

export class VADSilenceSegmentation implements SegmentationEngine {
  private messageCallbacks: Array<(message: string, metadata: SegmentMetadata) => void> = [];
  private segments: TranscriptSegment[] = [];
  private speaking = false;
  private silenceTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly silenceThresholdMs: number;

  constructor(config?: Partial<VADSilenceConfig>) {
    this.silenceThresholdMs = config?.silenceThresholdMs ?? 700;
  }

  onTranscript(segment: TranscriptSegment): void {
    this.segments.push(segment);
  }

  onVAD(speaking: boolean): void {
    this.speaking = speaking;

    if (speaking) {
      // User started speaking — cancel any pending silence timer
      if (this.silenceTimer) {
        clearTimeout(this.silenceTimer);
        this.silenceTimer = null;
      }
    } else {
      // User stopped speaking — start silence timer
      this.silenceTimer = setTimeout(() => {
        this.emitTurn();
      }, this.silenceThresholdMs);
    }
  }

  onMessage(cb: (message: string, metadata: SegmentMetadata) => void): void {
    this.messageCallbacks.push(cb);
  }

  flush(): string | null {
    if (this.silenceTimer) {
      clearTimeout(this.silenceTimer);
      this.silenceTimer = null;
    }
    return this.emitTurn();
  }

  private emitTurn(): string | null {
    if (this.segments.length === 0) {
      return null;
    }

    const text = this.segments.map((s) => s.text).join(" ").trim();
    if (!text) {
      this.segments = [];
      return null;
    }

    const metadata: SegmentMetadata = {
      startTime: this.segments[0].start,
      endTime: this.segments[this.segments.length - 1].end,
      segmentCount: this.segments.length,
    };

    this.segments = [];

    for (const cb of this.messageCallbacks) {
      cb(text, metadata);
    }

    return text;
  }
}
```

**Step 2: Commit**

```bash
git add extensions/noisy-claw/src/pipeline/segmentation/
git commit -m "feat: implement VAD silence segmentation engine"
```

---

## Task 12: Pipeline Coordinator

**Files:**
- Create: `extensions/noisy-claw/src/pipeline/coordinator.ts`

**Step 1: Implement the pipeline coordinator**

Wires all components together, manages echo cancellation state, and emits messages to the channel adapter.

```typescript
// src/pipeline/coordinator.ts

import type {
  AudioSource,
  STTProvider,
  SegmentationEngine,
  TTSProvider,
  AudioOutput,
  AudioConfig,
  STTConfig,
  SegmentMetadata,
} from "./interfaces.js";

export type PipelineConfig = {
  audio: AudioConfig;
  stt: STTConfig;
};

export type PipelineComponents = {
  audioSource: AudioSource;
  sttProvider: STTProvider;
  segmentation: SegmentationEngine;
  ttsProvider: TTSProvider;
  audioOutput: AudioOutput;
};

export class PipelineCoordinator {
  private readonly components: PipelineComponents;
  private messageCallbacks: Array<(message: string, metadata: SegmentMetadata) => void> = [];
  private active = false;
  private echoSuppressed = false;

  constructor(components: PipelineComponents) {
    this.components = components;
    this.wireComponents();
  }

  private wireComponents(): void {
    const { audioSource, sttProvider, segmentation, audioOutput } = this.components;

    // AudioSource VAD -> SegmentationEngine + echo cancel
    audioSource.onVAD((speaking) => {
      segmentation.onVAD(speaking);

      // Interruption: user speaks during playback
      if (speaking && this.echoSuppressed) {
        audioOutput.stop();
        this.echoSuppressed = false;
      }
    });

    // AudioSource audio chunks -> STTProvider (when not suppressed)
    audioSource.onAudio((chunk) => {
      if (!this.echoSuppressed) {
        sttProvider.feed(chunk);
      }
    });

    // STTProvider transcripts -> SegmentationEngine
    sttProvider.onTranscript((segment) => {
      segmentation.onTranscript(segment);
    });

    // SegmentationEngine messages -> callbacks
    segmentation.onMessage((message, metadata) => {
      for (const cb of this.messageCallbacks) {
        cb(message, metadata);
      }
    });

    // AudioOutput done -> un-suppress STT
    audioOutput.onDone(() => {
      this.echoSuppressed = false;
    });
  }

  start(config: PipelineConfig): void {
    if (this.active) return;
    this.active = true;
    this.components.audioSource.start(config.audio);
    this.components.sttProvider.start(config.stt);
  }

  stop(): void {
    if (!this.active) return;
    this.active = false;
    this.components.audioSource.stop();
    this.components.sttProvider.stop();
    // Flush remaining segments
    this.components.segmentation.flush();
  }

  async speak(text: string): Promise<void> {
    const audioPath = await this.components.ttsProvider.synthesize(text);
    this.echoSuppressed = true;
    await this.components.audioOutput.play(audioPath);
  }

  onMessage(cb: (message: string, metadata: SegmentMetadata) => void): void {
    this.messageCallbacks.push(cb);
  }

  get isActive(): boolean {
    return this.active;
  }

  get isSpeaking(): boolean {
    return this.components.audioOutput.isPlaying();
  }
}
```

**Step 2: Commit**

```bash
git add extensions/noisy-claw/src/pipeline/coordinator.ts
git commit -m "feat: implement pipeline coordinator with echo cancellation"
```

---

## Task 13: Voice Session Management

**Files:**
- Create: `extensions/noisy-claw/src/channel/session.ts`

**Step 1: Implement voice session state**

```typescript
// src/channel/session.ts

export type VoiceMode = "conversation" | "listen" | "dictation";

export type VoiceSessionState = {
  active: boolean;
  mode: VoiceMode;
  startTime: number | null;    // Unix timestamp ms
  segmentCount: number;
  currentlyListening: boolean;
  currentlySpeaking: boolean;
};

export class VoiceSession {
  private state: VoiceSessionState = {
    active: false,
    mode: "conversation",
    startTime: null,
    segmentCount: 0,
    currentlyListening: false,
    currentlySpeaking: false,
  };

  start(): VoiceSessionState {
    return {
      ...this.state,
      active: true,
      startTime: Date.now(),
      segmentCount: 0,
      currentlyListening: true,
    };
  }

  stop(): VoiceSessionState {
    return {
      ...this.state,
      active: false,
      startTime: null,
      currentlyListening: false,
      currentlySpeaking: false,
    };
  }

  setMode(mode: VoiceMode): VoiceSessionState {
    return { ...this.state, mode };
  }

  incrementSegments(): VoiceSessionState {
    return { ...this.state, segmentCount: this.state.segmentCount + 1 };
  }

  setSpeaking(speaking: boolean): VoiceSessionState {
    return { ...this.state, currentlySpeaking: speaking };
  }

  setListening(listening: boolean): VoiceSessionState {
    return { ...this.state, currentlyListening: listening };
  }

  getState(): Readonly<VoiceSessionState> {
    return this.state;
  }

  getDuration(): number {
    if (!this.state.startTime) return 0;
    return (Date.now() - this.state.startTime) / 1000;
  }

  update(next: VoiceSessionState): void {
    this.state = next;
  }
}
```

**Step 2: Commit**

```bash
git add extensions/noisy-claw/src/channel/session.ts
git commit -m "feat: implement voice session state management"
```

---

## Task 14: Agent Tools

**Files:**
- Create: `extensions/noisy-claw/src/tools/voice-mode.ts`
- Create: `extensions/noisy-claw/src/tools/voice-status.ts`

**Step 1: Implement voice_mode tool**

Follow the tool pattern from `openclaw/src/agents/tools/tts-tool.ts`:

```typescript
// src/tools/voice-mode.ts

import { Type } from "@sinclair/typebox";
import type { VoiceSession } from "../channel/session.js";

export function createVoiceModeTool(session: VoiceSession) {
  return {
    label: "Voice Mode",
    name: "voice_mode",
    description:
      "Switch the voice channel mode. Only 'conversation' is currently supported. " +
      "'listen' and 'dictation' modes will be available in a future release.",
    parameters: Type.Object({
      mode: Type.Union([
        Type.Literal("conversation"),
        Type.Literal("listen"),
        Type.Literal("dictation"),
      ], { description: "The voice channel mode to switch to." }),
    }),
    execute: async (_toolCallId: string, args: Record<string, unknown>) => {
      const mode = args.mode as "conversation" | "listen" | "dictation";

      if (mode !== "conversation") {
        return {
          content: [{ type: "text" as const, text: `Mode '${mode}' is not yet implemented. Only 'conversation' mode is available.` }],
        };
      }

      session.update(session.setMode(mode));

      return {
        content: [{ type: "text" as const, text: `Voice channel mode set to '${mode}'.` }],
      };
    },
  };
}
```

**Step 2: Implement voice_status tool**

```typescript
// src/tools/voice-status.ts

import { Type } from "@sinclair/typebox";
import type { VoiceSession } from "../channel/session.js";

export function createVoiceStatusTool(session: VoiceSession) {
  return {
    label: "Voice Status",
    name: "voice_status",
    description: "Get the current state of the voice channel session.",
    parameters: Type.Object({}),
    execute: async () => {
      const state = session.getState();

      const status = {
        active: state.active,
        mode: state.mode,
        duration: session.getDuration(),
        segmentCount: state.segmentCount,
        currentlyListening: state.currentlyListening,
        currentlySpeaking: state.currentlySpeaking,
      };

      return {
        content: [{ type: "text" as const, text: JSON.stringify(status, null, 2) }],
        details: status,
      };
    },
  };
}
```

**Step 3: Commit**

```bash
git add extensions/noisy-claw/src/tools/
git commit -m "feat: implement voice_mode and voice_status agent tools"
```

---

## Task 15: Channel Plugin — Config & Gateway Adapters

**Files:**
- Create: `extensions/noisy-claw/src/config/schema.ts`
- Create: `extensions/noisy-claw/src/config/defaults.ts`
- Create: `extensions/noisy-claw/src/channel/config.ts`
- Create: `extensions/noisy-claw/src/channel/gateway.ts`

**Step 1: Config schema (Zod)**

```typescript
// src/config/schema.ts

import { z } from "zod";

export const VoiceConfigSchema = z.object({
  enabled: z.boolean().optional(),
  mode: z.enum(["conversation", "listen", "dictation"]).optional(),
  audio: z.object({
    source: z.enum(["mic"]).optional(),
    sampleRate: z.number().optional(),
    device: z.string().optional(),
  }).optional(),
  stt: z.object({
    backend: z.enum(["whisper"]).optional(),
    model: z.string().optional(),
    language: z.string().optional(),
  }).optional(),
  tts: z.object({
    enabled: z.boolean().optional(),
  }).optional(),
  conversation: z.object({
    endOfTurnSilence: z.number().optional(),
    interruptible: z.boolean().optional(),
  }).optional(),
});

export type VoiceConfig = z.infer<typeof VoiceConfigSchema>;
```

**Step 2: Default config**

```typescript
// src/config/defaults.ts

import type { VoiceConfig } from "./schema.js";

export const DEFAULT_VOICE_CONFIG: Required<VoiceConfig> = {
  enabled: true,
  mode: "conversation",
  audio: {
    source: "mic",
    sampleRate: 16000,
    device: "default",
  },
  stt: {
    backend: "whisper",
    model: "base",
    language: "en",
  },
  tts: {
    enabled: true,
  },
  conversation: {
    endOfTurnSilence: 700,
    interruptible: true,
  },
};
```

**Step 3: Config adapter**

Follow the Matrix `config` adapter pattern:

```typescript
// src/channel/config.ts

import type { ChannelConfigAdapter } from "openclaw/plugin-sdk";
import { DEFAULT_VOICE_CONFIG } from "../config/defaults.js";
import type { VoiceConfig } from "../config/schema.js";

export type ResolvedVoiceAccount = {
  accountId: string;
  config: VoiceConfig;
};

export const voiceConfigAdapter: ChannelConfigAdapter<ResolvedVoiceAccount> = {
  listAccountIds: (_cfg) => {
    // Voice channel has a single implicit account
    return ["default"];
  },

  resolveAccount: (cfg, accountId) => {
    const voiceCfg = (cfg as any).channels?.voice ?? {};
    return {
      accountId: accountId ?? "default",
      config: { ...DEFAULT_VOICE_CONFIG, ...voiceCfg },
    };
  },

  defaultAccountId: () => "default",

  isEnabled: (account) => {
    return account.config.enabled !== false;
  },

  isConfigured: () => true, // No external service to configure

  describeAccount: (account) => ({
    id: account.accountId,
    status: "connected",
    label: "Voice (local mic)",
  }),
};
```

**Step 4: Gateway adapter**

```typescript
// src/channel/gateway.ts

import type { ChannelGatewayAdapter, ChannelGatewayContext } from "openclaw/plugin-sdk";
import type { ResolvedVoiceAccount } from "./config.js";
import { AudioSubprocess } from "../ipc/subprocess.js";
import { PipelineCoordinator, type PipelineComponents } from "../pipeline/coordinator.js";
import { RustLocalCapture } from "../pipeline/sources/rust-capture.js";
import { RustWhisperSTT } from "../pipeline/stt/rust-whisper.js";
import { RustLocalPlayback } from "../pipeline/output/rust-playback.js";
import { VADSilenceSegmentation } from "../pipeline/segmentation/vad-silence.js";
import { VoiceSession } from "./session.js";
import type { AudioEvent } from "../ipc/protocol.js";

// Module-level state (accessible to tools and outbound adapter)
let activePipeline: PipelineCoordinator | null = null;
let activeSession: VoiceSession | null = null;
let activeSubprocess: AudioSubprocess | null = null;

export function getActivePipeline(): PipelineCoordinator | null {
  return activePipeline;
}

export function getActiveSession(): VoiceSession | null {
  return activeSession;
}

export const voiceGatewayAdapter: ChannelGatewayAdapter<ResolvedVoiceAccount> = {
  startAccount: async (ctx: ChannelGatewayContext<ResolvedVoiceAccount>) => {
    const { account, abortSignal } = ctx;
    const config = account.config;

    // Resolve Rust binary path
    const binaryPath = resolveBinaryPath();

    // Create session
    const session = new VoiceSession();
    activeSession = session;

    // Create subprocess
    const capture = {} as RustLocalCapture;
    const sttProvider = {} as RustWhisperSTT;
    const playback = {} as RustLocalPlayback;

    const subprocess = new AudioSubprocess({
      binaryPath,
      onEvent: (event: AudioEvent) => {
        // Route events to the appropriate pipeline component
        if (event.event === "vad") {
          (capture as any).handleEvent?.(event);
        } else if (event.event === "transcript") {
          (sttProvider as any).handleEvent?.(event);
        } else if (event.event === "playback_done") {
          (playback as any).handleEvent?.(event);
        } else if (event.event === "error") {
          console.error(`[noisy-claw] audio engine error: ${(event as any).message}`);
        }
      },
      onError: (err) => {
        console.error(`[noisy-claw] subprocess error:`, err);
      },
      onExit: (code) => {
        console.log(`[noisy-claw] subprocess exited with code ${code}`);
        activePipeline = null;
        activeSubprocess = null;
      },
    });

    // Initialize pipeline components properly
    const rustCapture = new RustLocalCapture(subprocess);
    const rustSTT = new RustWhisperSTT(subprocess);
    const rustPlayback = new RustLocalPlayback(subprocess);
    const segmentation = new VADSilenceSegmentation({
      silenceThresholdMs: config.conversation?.endOfTurnSilence ?? 700,
    });

    // Update subprocess event routing to use actual instances
    subprocess.start();

    // Create TTS provider placeholder (injected from plugin registration)
    // The actual OpenClawTTS is set up in index.ts where we have access to the runtime
    const ttsProvider = (globalThis as any).__noisyClaw_ttsProvider;

    const components: PipelineComponents = {
      audioSource: rustCapture,
      sttProvider: rustSTT,
      segmentation,
      ttsProvider,
      audioOutput: rustPlayback,
    };

    const pipeline = new PipelineCoordinator(components);
    activePipeline = pipeline;
    activeSubprocess = subprocess;

    // Wire message callback to submit to OpenClaw's session manager
    // This will be connected in the plugin registration
    pipeline.onMessage((message, metadata) => {
      session.update(session.incrementSegments());
      // The actual message submission is handled by the messaging integration
      // set up in index.ts
    });

    // Don't start capture yet — wait for explicit activation
    ctx.setStatus({
      id: account.accountId,
      status: "connected",
      label: "Voice (ready, not capturing)",
    });

    // Wait for abort signal
    await new Promise<void>((resolve) => {
      abortSignal.addEventListener("abort", () => {
        subprocess.stop();
        pipeline.stop();
        activePipeline = null;
        activeSession = null;
        activeSubprocess = null;
        resolve();
      }, { once: true });
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
  // In production: bundled binary
  const devPath = new URL(
    "../../native/noisy-claw-audio/target/debug/noisy-claw-audio",
    import.meta.url,
  ).pathname;

  return devPath;
}
```

**Step 5: Commit**

```bash
git add extensions/noisy-claw/src/config/ extensions/noisy-claw/src/channel/config.ts extensions/noisy-claw/src/channel/gateway.ts
git commit -m "feat: implement channel config, gateway adapters, and voice config schema"
```

---

## Task 16: Channel Plugin — Outbound Adapter & Plugin Definition

**Files:**
- Create: `extensions/noisy-claw/src/channel/outbound.ts`
- Create: `extensions/noisy-claw/src/channel/plugin.ts`

**Step 1: Outbound adapter (dual delivery: text + TTS)**

```typescript
// src/channel/outbound.ts

import type { ChannelOutboundAdapter } from "openclaw/plugin-sdk";
import { getActivePipeline } from "./gateway.js";

export const voiceOutboundAdapter: ChannelOutboundAdapter = {
  deliveryMode: "direct",
  textChunkLimit: 4000,

  sendText: async ({ text }) => {
    // 1. Text is already delivered via the normal channel path (Gateway handles this)
    // 2. Additionally, synthesize and play audio
    const pipeline = getActivePipeline();
    if (pipeline) {
      try {
        await pipeline.speak(text);
      } catch (err) {
        console.error("[noisy-claw] TTS playback failed:", err);
      }
    }

    return {
      channel: "voice",
      messageId: `voice-${Date.now()}`,
    };
  },

  sendMedia: async ({ text }) => {
    // Voice channel doesn't support media — just read the text caption
    if (text) {
      const pipeline = getActivePipeline();
      if (pipeline) {
        await pipeline.speak(text);
      }
    }
    return {
      channel: "voice",
      messageId: `voice-${Date.now()}`,
    };
  },
};
```

**Step 2: Plugin definition**

```typescript
// src/channel/plugin.ts

import type { ChannelPlugin } from "openclaw/plugin-sdk";
import { voiceConfigAdapter, type ResolvedVoiceAccount } from "./config.js";
import { voiceGatewayAdapter } from "./gateway.js";
import { voiceOutboundAdapter } from "./outbound.js";
import { VoiceConfigSchema } from "../config/schema.js";

export const voiceChannelPlugin: ChannelPlugin<ResolvedVoiceAccount> = {
  id: "voice",

  meta: {
    id: "voice",
    label: "Voice",
    selectionLabel: "Voice (noisy-claw)",
    docsPath: "/channels/voice",
    docsLabel: "voice",
    blurb: "bidirectional voice channel; speak to your agent, hear it respond.",
    order: 80,
    quickstartAllowFrom: false,
  },

  capabilities: {
    chatTypes: ["direct"],
    polls: false,
    reactions: false,
    threads: false,
    media: false,
  },

  reload: { configPrefixes: ["channels.voice"] },

  config: voiceConfigAdapter,

  gateway: voiceGatewayAdapter,

  outbound: voiceOutboundAdapter,

  messaging: {
    normalizeTarget: (raw) => raw.startsWith("voice:") ? raw : `voice:${raw}`,
    targetResolver: {
      looksLikeId: (raw) => raw.startsWith("voice:"),
      hint: "voice:<session-id>",
    },
  },
};
```

**Step 3: Commit**

```bash
git add extensions/noisy-claw/src/channel/outbound.ts extensions/noisy-claw/src/channel/plugin.ts
git commit -m "feat: implement outbound adapter and channel plugin definition"
```

---

## Task 17: Plugin Entry — Registration

**Files:**
- Modify: `extensions/noisy-claw/src/index.ts`

**Step 1: Replace stub with full plugin registration**

```typescript
// src/index.ts

import type { OpenClawPluginApi } from "openclaw/plugin-sdk";
import { emptyPluginConfigSchema } from "openclaw/plugin-sdk";
import { voiceChannelPlugin } from "./channel/plugin.js";
import { createVoiceModeTool } from "./tools/voice-mode.js";
import { createVoiceStatusTool } from "./tools/voice-status.js";
import { VoiceSession } from "./channel/session.js";

const plugin = {
  id: "noisy-claw",
  name: "Noisy Claw",
  description: "Voice channel plugin — bidirectional voice as a first-class channel",
  configSchema: emptyPluginConfigSchema(),

  register(api: OpenClawPluginApi) {
    console.log("[noisy-claw] registering voice channel plugin");

    // Register the channel
    api.registerChannel({ plugin: voiceChannelPlugin });

    // Create a shared session instance for tools
    const session = new VoiceSession();

    // Register agent tools
    api.registerTool(() => createVoiceModeTool(session));
    api.registerTool(() => createVoiceStatusTool(session));

    console.log("[noisy-claw] voice channel plugin registered");
  },
};

export default plugin;
```

**Step 2: Verify TypeScript compiles**

```bash
cd /path/to/openclaw && pnpm exec tsc --noEmit
```

Expected: no type errors in the noisy-claw extension.

**Step 3: Commit**

```bash
git add extensions/noisy-claw/src/index.ts
git commit -m "feat: implement plugin entry with channel and tool registration"
```

---

## Task 18: SKILL.md — Agent Voice Skill

**Files:**
- Create: `extensions/noisy-claw/skills/SKILL.md`

**Step 1: Write the agent skill**

```markdown
---
name: voice-channel
description: Control the voice channel — start/stop listening, switch modes, check status
activation: auto
---

# Voice Channel

You have access to a voice channel that lets users speak to you and hear your responses.

## Tools

- `voice_mode`: Switch the voice channel mode (conversation/listen/dictation). Currently only "conversation" is supported.
- `voice_status`: Check if the voice channel is active, what mode it's in, and whether you're currently listening or speaking.

## Behavior

When the voice channel is active:
- User speech is automatically transcribed and delivered to you as text messages
- Your text responses are automatically converted to speech and played back to the user
- The text version of your response is also visible in the chat
- If the user speaks while you're responding, playback stops and a new turn begins

## Guidelines

- Keep voice responses concise — aim for 1-3 sentences when possible
- Avoid markdown formatting in voice responses (it won't be spoken)
- Don't reference visual elements the user can't see
- If a response requires detailed information (code, lists, links), suggest the user check the text version
```

**Step 2: Commit**

```bash
git add extensions/noisy-claw/skills/
git commit -m "feat: add voice channel agent skill documentation"
```

---

## Task 19: Build & Smoke Test

**Step 1: Build the Rust binary**

```bash
cd extensions/noisy-claw/native/noisy-claw-audio && cargo build --release
```

Expected: compiles to `target/release/noisy-claw-audio`.

**Step 2: Download models**

```bash
mkdir -p extensions/noisy-claw/models
# Silero VAD model
curl -L -o extensions/noisy-claw/models/silero_vad.onnx \
  https://github.com/snakers4/silero-vad/raw/master/files/silero_vad.onnx
# Whisper base model
curl -L -o extensions/noisy-claw/models/ggml-base.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin
```

**Step 3: Smoke test the Rust binary**

```bash
echo '{"cmd":"get_status"}' | ./target/release/noisy-claw-audio
```

Expected output: `{"event":"ready"}` followed by `{"event":"status","capturing":false,"playing":false}`.

**Step 4: Build the TypeScript extension**

```bash
cd /path/to/openclaw && pnpm build
```

Expected: builds without errors, extensions/noisy-claw is compiled to `dist/extensions/noisy-claw/`.

**Step 5: Add models to .gitignore**

```bash
echo "models/" >> extensions/noisy-claw/.gitignore
echo "native/noisy-claw-audio/target/" >> extensions/noisy-claw/.gitignore
```

**Step 6: Commit**

```bash
git add extensions/noisy-claw/.gitignore
git commit -m "chore: add .gitignore for models and Rust build output"
```

---

## Task 20: Integration Wiring & Manual Test

**Step 1: Update gateway adapter to properly route events**

The gateway adapter in Task 15 has placeholder event routing. Now that all components exist, verify the event routing is correct by reviewing the subprocess event handler and ensuring `handleEvent` is called on the right component instances.

**Step 2: Test the full flow manually**

1. Start OpenClaw gateway with noisy-claw extension enabled
2. Configure voice channel in `openclaw.json`:
   ```json
   { "channels": { "voice": { "enabled": true } } }
   ```
3. Start the voice channel (via CLI command or Gateway API)
4. Speak into the mic
5. Verify: transcript appears in Gateway logs
6. Verify: agent responds with text + TTS audio playback
7. Verify: speaking during TTS playback interrupts it

**Step 3: Fix any integration issues found during manual testing**

Common issues to check:
- Binary path resolution (development vs production)
- Model file paths
- Audio device permissions on macOS (microphone access)
- TTS provider configuration
- IPC protocol field naming mismatches between Rust and TypeScript

**Step 4: Final commit**

```bash
git add -A
git commit -m "feat: complete Phase 1 MVP — voice channel with conversation mode"
```

---

## Dependency Graph

```
Task 1 (TS scaffold) ──────────────────────────────────────┐
Task 2 (Rust scaffold) ────────────────────────────────────┤
Task 3 (TS IPC protocol) ─── depends on Task 1 ───────────┤
Task 4 (TS pipeline interfaces) ─── depends on Task 1 ────┤
                                                           │
Task 5 (Rust capture) ─── depends on Task 2 ──────────────┤
Task 6 (Rust VAD) ─── depends on Task 2 ───────────────────┤
Task 7 (Rust STT) ─── depends on Task 2 ───────────────────┤
Task 8 (Rust playback) ─── depends on Task 2 ──────────────┤
Task 9 (Rust main loop) ─── depends on 5,6,7,8 ───────────┤
                                                           │
Task 10 (TS pipeline impls) ─── depends on 3,4 ────────────┤
Task 11 (Segmentation) ─── depends on 4 ───────────────────┤
Task 12 (Coordinator) ─── depends on 4,10,11 ──────────────┤
Task 13 (Session) ─── depends on 1 ────────────────────────┤
Task 14 (Agent tools) ─── depends on 13 ───────────────────┤
Task 15 (Config & Gateway) ─── depends on 3,10,11,12,13 ──┤
Task 16 (Outbound & Plugin def) ─── depends on 12,15 ─────┤
Task 17 (Plugin entry) ─── depends on 14,16 ───────────────┤
Task 18 (SKILL.md) ─── no deps ────────────────────────────┤
Task 19 (Build & smoke test) ─── depends on 9,17 ──────────┤
Task 20 (Integration test) ─── depends on 19 ──────────────┘
```

**Parallel execution opportunities:**
- Tasks 1 & 2 (TS + Rust scaffold) → parallel
- Tasks 5, 6, 7, 8 (Rust modules) → parallel after Task 2
- Tasks 3 & 4 (IPC protocol + interfaces) → parallel after Task 1
- Tasks 10, 11, 13, 18 → parallel after their deps
