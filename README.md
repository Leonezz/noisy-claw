# Noisy Claw

OpenClaw voice channel plugin — bidirectional voice as a first-class channel.

Noisy Claw captures audio from your microphone, detects speech with Silero VAD, transcribes it with Whisper, and delivers the text to the OpenClaw agent. Agent responses are delivered back through the voice channel (TTS support is planned for a future release).

## Architecture

```
Microphone
    |
    v
┌───────────────────────────────────────────┐
│  Rust audio engine (noisy-claw-audio)     │
│  capture → resample → VAD → STT (Whisper) │
└───────────────┬───────────────────────────┘
                │ IPC (JSON over stdin/stdout)
                v
┌───────────────────────────────────────────┐
│  TypeScript plugin layer                  │
│  pipeline coordinator → session → agent   │
└───────────────────────────────────────────┘
```

The Rust binary handles all real-time audio processing. The TypeScript layer manages session state, pipeline coordination, and integration with the OpenClaw plugin SDK.

## Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain)
- Node.js 22+
- pnpm (workspace root uses pnpm)
- A working microphone

## Setup

### 1. Download models

The audio engine requires two model files placed in `extensions/noisy-claw/models/`:

```bash
mkdir -p extensions/noisy-claw/models

# Silero VAD v5 (~2.3 MB)
curl -L -o extensions/noisy-claw/models/silero_vad.onnx \
  "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx"

# Whisper base model (~141 MB)
curl -L -o extensions/noisy-claw/models/ggml-base.bin \
  "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"
```

### 2. Build the Rust audio engine

```bash
cd extensions/noisy-claw/native/noisy-claw-audio
cargo build --release
```

The binary is built to `target/release/noisy-claw-audio`.

### 3. Install dependencies

From the repository root:

```bash
pnpm install
pnpm build
```

### 4. Configure OpenClaw

Add the voice channel to your OpenClaw config at `~/.openclaw/openclaw.json`:

```json
{
  "plugins": {
    "enabled": true
  },
  "channels": {
    "voice": {
      "enabled": true
    }
  }
}
```

### 5. Run the gateway

```bash
node --import tsx dist/entry.js gateway --dev
```

You should see:

```
[noisy-claw] No TTS provider injected — voice responses will be text-only
[noisy-claw-audio] capture started
```

The plugin is now listening on your microphone. Speak and the transcribed text will be delivered to the agent.

## Configuration

All fields are optional. Defaults are shown below:

```json
{
  "channels": {
    "voice": {
      "enabled": true,
      "mode": "conversation",
      "audio": {
        "source": "mic",
        "sampleRate": 16000,
        "device": "default"
      },
      "stt": {
        "backend": "whisper",
        "model": "base",
        "language": "en"
      },
      "tts": {
        "enabled": true
      },
      "conversation": {
        "endOfTurnSilence": 700,
        "interruptible": true
      }
    }
  }
}
```

| Field                           | Description                                                                   |
| ------------------------------- | ----------------------------------------------------------------------------- |
| `audio.device`                  | Input device name, or `"default"` for system default                          |
| `audio.sampleRate`              | Target sample rate in Hz. The capture resamples from the device's native rate |
| `stt.model`                     | Whisper model size: `tiny`, `base`, `small`, `medium`, `large`                |
| `stt.language`                  | Language code for transcription (e.g., `en`, `zh`, `ja`)                      |
| `conversation.endOfTurnSilence` | Milliseconds of silence before a turn is considered complete                  |

## Agent tools

The plugin registers two tools that the agent can use:

- **`voice_mode`** — Switch the voice channel mode. Only `conversation` mode is implemented; `listen` and `dictation` modes are planned.
- **`voice_status`** — Get the current state of the voice session (active, mode, duration, segment count).

## Running tests

```bash
# TypeScript tests (from repo root)
npx vitest run extensions/noisy-claw

# Rust tests
cd extensions/noisy-claw/native/noisy-claw-audio
cargo test
```

## Project structure

```
extensions/noisy-claw/
  index.ts                  # Plugin entry point
  openclaw.plugin.json      # Plugin manifest
  models/                   # VAD and STT model files (not checked in)
  native/noisy-claw-audio/  # Rust audio engine
    src/
      main.rs               # IPC command loop, pipeline orchestration
      capture.rs            # Audio capture with resample and channel mixing
      vad.rs                # Silero VAD v5 inference
      stt.rs                # Whisper STT
      playback.rs           # Audio playback
      protocol.rs           # IPC message types
  src/
    channel/
      config.ts             # Voice config adapter
      gateway.ts            # Gateway adapter (subprocess lifecycle)
      outbound.ts           # Outbound adapter (TTS delivery)
      plugin.ts             # Channel plugin definition
      session.ts            # Voice session state machine
    config/
      schema.ts             # Zod config schema
      defaults.ts           # Default configuration values
    ipc/
      protocol.ts           # TypeScript IPC protocol types
      subprocess.ts         # Subprocess manager
    pipeline/
      coordinator.ts        # Pipeline coordinator
      interfaces.ts         # Pipeline component interfaces
      sources/              # Audio source implementations
      segmentation/         # VAD-based segmentation
      stt/                  # STT provider implementations
      tts/                  # TTS provider implementations
      output/               # Audio output implementations
    tools/
      voice-mode.ts         # voice_mode agent tool
      voice-status.ts       # voice_status agent tool
```

## Current limitations

- **TTS is not wired** — Agent responses are text-only. The outbound adapter is ready for a TTS provider but none is injected yet.
- **Only `conversation` mode** — `listen` and `dictation` modes return "not yet implemented".
- **Linear interpolation resampler** — Adequate for voice but not production-grade for music or wide-band audio.
- **No echo cancellation** — If using speakers instead of headphones, the agent's TTS output may be picked up by the microphone.
