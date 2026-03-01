# Noisy-Claw Phase 1 MVP — Implementation Design

> **Date**: 2026-02-28
> **Status**: Approved
> **Scope**: Conversation mode voice channel for OpenClaw (macOS only)

## Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Build approach | OpenClaw extension (real interfaces) | Build directly against real plugin SDK |
| Target platform | macOS only | Simplify Phase 1; Linux in Phase 2+ |
| Native language | Rust | Cross-platform portability for later phases |
| Audio architecture | Monolithic Rust subprocess | Low-latency pipeline, crash isolation from Gateway |
| STT backend | whisper.cpp (via whisper-rs in Rust) | Local, no API key, battle-tested |
| TTS | OpenClaw's existing infrastructure | Reuse ElevenLabs/OpenAI/Edge providers |
| Wake word | Simple keyword matching on STT output | No separate ML model needed for Phase 1 |
| Activation | Explicit (command/button) | No surprise mic access |
| Echo cancellation | Suppress STT during playback, VAD stays on | Enables interruption detection |
| Output | Dual: text + audio | Text is source of truth, audio is bonus delivery |

## Architecture

Two main components:

### 1. TypeScript Plugin (`extensions/noisy-claw/`)

Implements OpenClaw's `ChannelPlugin` interface. Owns:
- Channel registration, config, session management
- **Pipeline orchestration** with pluggable interfaces
- Agent tools (`voice_mode`, `voice_status`)
- Subprocess lifecycle management
- Routes agent text responses to text output + TTS + playback

### 2. Rust Audio Engine (`native/noisy-claw-audio/`)

Single binary handling:
- Mic capture via `cpal`
- Voice Activity Detection via Silero VAD (ONNX runtime)
- STT via `whisper-rs` (whisper.cpp bindings)
- Audio playback via `rodio`

### IPC Protocol (JSON lines over stdin/stdout)

```
Node -> Rust:  {"cmd": "start_capture", "device": "default", "sample_rate": 16000}
Node -> Rust:  {"cmd": "play_audio", "path": "/tmp/tts-output.mp3"}
Node -> Rust:  {"cmd": "stop_capture"}
Node -> Rust:  {"cmd": "stop_playback"}

Rust -> Node:  {"event": "audio_chunk", "data": "<base64 PCM>", "timestamp": 1.234}
Rust -> Node:  {"event": "vad", "speaking": true}
Rust -> Node:  {"event": "transcript", "text": "Hello world", "is_final": true, "start": 0.0, "end": 1.2}
Rust -> Node:  {"event": "playback_done"}
Rust -> Node:  {"event": "error", "message": "Device not found"}
```

## Pluggable Pipeline

All pipeline stages are behind abstract interfaces, allowing swap-in of alternative implementations (e.g., cloud STT, semantic segmentation).

```
AudioSource --> STTProvider --> SegmentationEngine --> message
     |
     +--> VAD (always on) --> SegmentationEngine
                           --> echo cancel coordinator

TTSProvider <-- agent response
     |
     v
AudioOutput --> echo cancel coordinator
                (suppresses STT feed, VAD stays active)
```

### Interfaces

```typescript
interface AudioSource {
  start(config: AudioConfig): void;
  stop(): void;
  onAudio(cb: (chunk: AudioChunk) => void): void;
  onVAD(cb: (speaking: boolean) => void): void;
}

interface STTProvider {
  start(config: STTConfig): void;
  stop(): void;
  feed(chunk: AudioChunk): void;
  onTranscript(cb: (segment: TranscriptSegment) => void): void;
}

interface SegmentationEngine {
  onTranscript(segment: TranscriptSegment): void;
  onVAD(speaking: boolean): void;
  onMessage(cb: (message: string, metadata: SegmentMetadata) => void): void;
  flush(): string | null;
}

interface TTSProvider {
  synthesize(text: string, opts?: TTSOpts): Promise<string>;
}

interface AudioOutput {
  play(audioPath: string): Promise<void>;
  stop(): void;
  isPlaying(): boolean;
}
```

### Phase 1 Implementations

| Interface | Implementation | Backend |
|---|---|---|
| AudioSource | RustLocalCapture | Rust subprocess (cpal) |
| STTProvider | RustWhisperSTT | Rust subprocess (whisper-rs) |
| SegmentationEngine | VADSilenceSegmentation | VAD + silence threshold (700ms) |
| TTSProvider | OpenClawTTS | OpenClaw's textToSpeech() |
| AudioOutput | RustLocalPlayback | Rust subprocess (rodio) |

## Conversation Mode Flow

```
User speaks
  -> AudioSource captures mic PCM
  -> VAD detects speech start
  -> STTProvider transcribes audio
  -> User stops speaking
  -> VAD detects silence > 700ms
  -> SegmentationEngine emits complete utterance
  -> Plugin creates MsgContext {Body, Provider: "voice", ChatType: "direct"}
  -> Submitted to OpenClaw session manager
  -> Normal agent pipeline processes

Agent responds
  -> Text delivered via normal channel path (visible in UI/logs)
  -> Text -> TTSProvider -> audio file
  -> AudioOutput plays audio
  -> Echo cancel: STT feed suppressed, VAD still running

User interrupts during TTS
  -> VAD detects speech during playback
  -> AudioOutput.stop() immediately
  -> STT feed resumed
  -> New turn begins
```

## OpenClaw Integration

### Channel Plugin Adapters (Phase 1)

| Adapter | Implementation |
|---|---|
| config | Audio device selection, STT/TTS provider config |
| gateway | startAccount spawns Rust subprocess, stopAccount kills it |
| messaging | Voice session ID as target |
| outbound | Dual delivery: text + TTS audio playback |
| capabilities | chatTypes: ["direct"], no media/polls/threads |
| agentTools | voice_mode, voice_status |

### Agent Tools

```typescript
// voice_mode: switch channel modes (only "conversation" in Phase 1)
voice_mode(mode: "conversation" | "listen" | "dictation"): void

// voice_status: get current voice session state
voice_status(): {
  active: boolean;
  mode: string;
  duration: number;
  currentlyListening: boolean;
  currentlySpeaking: boolean;
}
```

## Project Structure

```
extensions/noisy-claw/
├── package.json
├── tsconfig.json
├── src/
│   ├── index.ts                    # Plugin entry
│   ├── channel/
│   │   ├── plugin.ts               # ChannelPlugin definition
│   │   ├── config.ts               # ChannelConfigAdapter
│   │   ├── gateway.ts              # ChannelGatewayAdapter
│   │   ├── outbound.ts             # ChannelOutboundAdapter
│   │   └── session.ts              # Voice session state
│   ├── pipeline/
│   │   ├── interfaces.ts           # All pluggable interfaces
│   │   ├── coordinator.ts          # Pipeline orchestrator + echo cancel
│   │   ├── sources/
│   │   │   └── rust-capture.ts     # AudioSource: Rust subprocess
│   │   ├── stt/
│   │   │   └── rust-whisper.ts     # STTProvider: Rust subprocess
│   │   ├── segmentation/
│   │   │   └── vad-silence.ts      # SegmentationEngine: VAD + silence
│   │   ├── tts/
│   │   │   └── openclaw-tts.ts     # TTSProvider: OpenClaw wrapper
│   │   └── output/
│   │       └── rust-playback.ts    # AudioOutput: Rust subprocess
│   ├── tools/
│   │   ├── voice-mode.ts
│   │   └── voice-status.ts
│   ├── ipc/
│   │   ├── protocol.ts             # IPC message types
│   │   └── subprocess.ts           # Rust subprocess lifecycle
│   └── config/
│       ├── schema.ts               # Config schema (TypeBox)
│       └── defaults.ts             # Defaults
├── native/
│   └── noisy-claw-audio/
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs             # Entry, IPC handler
│           ├── capture.rs          # cpal mic capture
│           ├── vad.rs              # Silero VAD (ONNX)
│           ├── stt.rs              # whisper-rs STT
│           ├── playback.rs         # rodio playback
│           └── protocol.rs         # IPC types (serde)
├── models/                         # Downloaded on first use
│   ├── silero_vad.onnx
│   └── ggml-base.bin
└── skills/
    └── SKILL.md
```
