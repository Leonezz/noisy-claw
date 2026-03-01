# Noisy Claw



https://github.com/user-attachments/assets/777d9c17-a3e3-460f-be79-5ebf01b203b8



OpenClaw voice channel plugin — bidirectional voice as a first-class channel.

Speak to your agent, hear it respond. Noisy Claw captures audio from your microphone, detects speech with Silero VAD, transcribes it (cloud or local), and delivers the text to the OpenClaw agent. Agent responses are streamed sentence-by-sentence through TTS and played back in real time, with echo cancellation and barge-in support.

## Features

- **Streaming TTS** — agent responses are split at sentence boundaries and synthesized as they arrive, minimizing time-to-first-audio
- **Barge-in** — speak over the agent to interrupt; playback stops and a new turn begins
- **Echo cancellation** — WebRTC AEC3 removes speaker output from the microphone signal so the system doesn't hear itself
- **Cloud STT/TTS** — Aliyun DashScope via WebSocket (paraformer-realtime-v2 for STT, cosyvoice-v3-flash for TTS)
- **Local STT fallback** — Whisper.cpp when no cloud provider is configured
- **Agent tools** — `voice_speak`, `voice_listen`, `voice_mode`, `voice_status`
- **VAD-based turn detection** — configurable silence threshold for end-of-turn detection

## Architecture

```
TypeScript (OpenClaw plugin)           Rust (native audio engine)
┌────────────────────────────┐        ┌─────────────────────────────┐
│  index.ts                  │        │  noisy-claw-audio           │
│  ├─ channel/gateway        │ stdin  │  ├─ capture (cpal)          │
│  ├─ channel/dispatch       │ ─────► │  ├─ VAD (Silero ONNX)       │
│  ├─ pipeline/coordinator   │ stdout │  ├─ AEC (WebRTC AEC3)       │
│  ├─ pipeline/output        │ ◄───── │  ├─ output (cpal ring buf)  │
│  ├─ ipc/subprocess         │  JSON  │  ├─ cloud STT/TTS (WS)      │
│  └─ tools/*                │        │  └─ local STT (Whisper)     │
└────────────────────────────┘        └─────────────────────────────┘
```

The Rust binary handles all real-time audio: capture, resampling, VAD, echo cancellation, STT, TTS synthesis, and playback. The TypeScript layer manages the OpenClaw plugin lifecycle, pipeline coordination, sentence-boundary dispatch, and integration with the plugin SDK. Communication is JSON-over-stdio.

## Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain)
- Node.js 20+
- pnpm
- macOS (CoreAudio — Linux/Windows support planned)

## Setup

### 1. Install dependencies

```bash
pnpm install
```

### 2. Build the Rust audio engine

```bash
cd native/noisy-claw-audio
cargo build --release
```

### 3. Models

Models (Silero VAD, optionally Whisper base) are auto-downloaded on first run via the `noisy-claw-models` service. No manual download is needed.

To force a manual download:

```bash
openclaw voice setup
```

### 4. Configure OpenClaw

Add the voice channel to your OpenClaw config:

```json
{
  "channels": {
    "voice": {
      "enabled": true
    }
  }
}
```

## Configuration

All fields are optional. Shown below with defaults:

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
        "provider": "aliyun",
        "model": "paraformer-realtime-v2",
        "languages": ["zh", "en"],
        "apiKey": "...",
        "endpoint": null,
        "extra": {}
      },
      "tts": {
        "enabled": true,
        "provider": "aliyun",
        "model": "cosyvoice-v3-flash",
        "voice": "longanyang",
        "sampleRate": 16000,
        "speed": 1.0
      },
      "conversation": {
        "endOfTurnSilence": 700,
        "interruptible": true
      }
    }
  }
}
```

| Field | Description |
|-------|-------------|
| `audio.device` | Input device name, or `"default"` for system default |
| `audio.sampleRate` | Capture sample rate in Hz (device resamples to this) |
| `stt.provider` | `"aliyun"` for cloud, `"whisper"` for local |
| `stt.model` | STT model name (cloud: `paraformer-realtime-v2`; local: `base`, `small`, etc.) |
| `stt.languages` | Language hints for cloud STT (e.g., `["zh", "en"]`) |
| `stt.apiKey` | API key for cloud STT (or set `DASHSCOPE_API_KEY` env var) |
| `tts.provider` | TTS provider (currently `"aliyun"`) |
| `tts.model` | TTS model name (e.g., `cosyvoice-v3-flash`) |
| `tts.voice` | Voice name (e.g., `longanyang`) |
| `tts.speed` | Speech speed multiplier |
| `conversation.endOfTurnSilence` | Milliseconds of silence before a turn is complete |
| `conversation.interruptible` | Allow barge-in during agent speech |

## Agent Tools

The plugin registers four tools available to the agent:

| Tool | Description |
|------|-------------|
| `voice_speak` | Synthesize and play text aloud (independent of the agent's text reply) |
| `voice_listen` | Start or stop microphone listening |
| `voice_mode` | Switch voice channel mode (currently only `conversation`) |
| `voice_status` | Query session state: active, mode, duration, segment count, listening, speaking |

## IPC Protocol

Commands (TypeScript -> Rust, JSON per line on stdin):

| Command | Description |
|---------|-------------|
| `start_capture` | Begin mic capture with optional cloud STT config |
| `stop_capture` | Stop mic capture |
| `speak` | Synthesize and play full text (batch mode) |
| `speak_start` | Begin streaming TTS session |
| `speak_chunk` | Send text chunk for synthesis |
| `speak_end` | End streaming TTS session |
| `stop_speaking` | Interrupt TTS playback |
| `play_audio` | Play a pre-recorded audio file |
| `stop_playback` | Stop file playback |
| `get_status` | Query engine state |
| `shutdown` | Terminate engine |

Events (Rust -> TypeScript, JSON per line on stdout):

| Event | Description |
|-------|-------------|
| `ready` | Engine initialized |
| `vad` | Voice activity: `{speaking: bool}` |
| `transcript` | STT result: `{text, is_final, start, end, confidence?}` |
| `speak_started` | TTS synthesis began |
| `speak_done` | TTS playback completed |
| `playback_done` | File playback completed |
| `status` | Current state: `{capturing, playing, speaking}` |
| `error` | Error message |

## Development

```bash
# TypeScript tests
npx vitest run

# Rust tests
cd native/noisy-claw-audio
cargo test

# Run with debug logging
RUST_LOG=noisy_claw_audio=debug cargo run --release
```

## Project Structure

```
index.ts                      # Plugin entry point
skills/SKILL.md               # Agent-facing skill description
native/noisy-claw-audio/      # Rust audio engine
  src/
    main.rs                   # IPC loop, pipeline orchestration
    capture.rs                # Audio capture (cpal), resample, channel mixing
    vad.rs                    # Silero VAD v5 inference (ONNX)
    aec.rs                    # Echo cancellation (WebRTC AEC3 at 48kHz)
    output.rs                 # Streaming output (cpal ring buffer + AEC ref tap)
    stt.rs                    # Local STT (Whisper.cpp)
    playback.rs               # File-based audio playback
    audio_utils.rs            # Resample, PCM conversion utilities
    protocol.rs               # IPC command/event types
    cloud/
      traits.rs               # SpeechRecognizer, StreamingSpeechSynthesizer, TtsSession
      aliyun/
        dashscope_stt.rs      # DashScope real-time ASR (WebSocket)
        dashscope_tts.rs      # DashScope streaming TTS (WebSocket)
        dashscope_protocol.rs # DashScope wire protocol helpers
src/
  channel/
    config.ts                 # Voice config adapter
    gateway.ts                # Gateway adapter (subprocess lifecycle)
    dispatch.ts               # Sentence-boundary dispatch for streaming TTS
    outbound.ts               # Outbound adapter (routes agent replies to TTS)
    plugin.ts                 # Channel plugin definition
    session.ts                # Voice session state machine
  config/
    schema.ts                 # Zod config schema
    defaults.ts               # Default configuration values
  ipc/
    protocol.ts               # TypeScript IPC protocol types
    subprocess.ts             # Subprocess manager
  pipeline/
    coordinator.ts            # Pipeline coordinator (VAD, echo suppression, barge-in)
    interfaces.ts             # Pipeline component interfaces
    sources/                  # Audio source implementations
    segmentation/             # VAD-based turn segmentation
    output/                   # Audio output (RustLocalPlayback)
  tools/
    voice-speak.ts            # voice_speak agent tool
    voice-listen.ts           # voice_listen agent tool
    voice-mode.ts             # voice_mode agent tool
    voice-status.ts           # voice_status agent tool
  models/
    manager.ts                # Model auto-download
  cli.ts                      # CLI commands (openclaw voice setup/models)
```

## Current Limitations

- **Only `conversation` mode** — `listen` and `dictation` modes return "not yet implemented"
- **Only Aliyun DashScope** — cloud provider architecture is trait-based and extensible, but only Aliyun is implemented
- **macOS only** — uses CoreAudio via cpal; Linux ALSA/PulseAudio and Windows WASAPI should work but are untested
- **Linear interpolation resampler** — adequate for voice but has no anti-aliasing filter; AEC runs at 48kHz to avoid downsampling the render reference
