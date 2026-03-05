# Pipeline Refactor Design

**Date:** 2026-03-03
**Branch:** refactor/pipeline-nodes
**Approach:** Option B — Trait-based pipeline refactor with future graph engine path

## Goals

1. Fix overlapping audio (ring buffer not cleared on interrupt)
2. Fix wrong STT words during interruption (echoSuppressed gate drops audio)
3. Add request_id/flush_id tracking (prevent stale audio)
4. Add sentence-boundary chunking (faster first audio, cleaner interrupts)
5. Introduce PipelineNode trait + FlushProtocol for composability
6. Simplify TypeScript layer (remove false abstractions)

## Architecture Decision

**Rust owns the audio pipeline.** TypeScript is a thin IPC orchestrator that handles conversation turn management, LLM calls, and sentence chunking. The IPC protocol is the real API contract.

---

## Section 1: Rust Pipeline Message Types

### New types in `pipeline/mod.rs`

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeId { Capture, Aec, Vad, Stt, Tts, Output }

#[derive(Clone, Debug, PartialEq)]
pub struct RequestId(pub String);

pub enum FlushSignal {
    Flush { request_id: String },
    FlushAll,
}

pub struct FlushAck {
    pub node: NodeId,
    pub request_id: Option<String>,
}
```

### Revised OutputMessage

```rust
pub enum OutputMessage {
    StartSession { request_id: RequestId, sample_rate: u32 },
    AudioChunk { request_id: RequestId, samples: Vec<f32>, sample_rate: u32 },
    FinishSession { request_id: RequestId },
    StopSession { request_id: RequestId },
    StopAll,
}
```

### PipelineNode Trait

```rust
#[async_trait]
pub trait PipelineNode: Send + 'static {
    fn node_id(&self) -> NodeId;
    async fn flush(&mut self, signal: FlushSignal) -> FlushAck;
    async fn shutdown(&mut self);
}
```

All nodes implement this trait. Domain-specific commands remain in per-node `Control` enums. The trait enables future graph-based orchestration.

---

## Section 2: Bug Fixes

### 2a. Ring Buffer Flush on Interrupt

In `output.rs` StopSession/StopAll handler:
1. Call stop() on StreamingOutput
2. Drop the StreamingOutput entirely (set to None) — releases ring buffer
3. Next StartSession creates fresh StreamingOutput

Additionally: output node checks request_id on every AudioChunk. Mismatched chunks are silently dropped.

### 2b. Remove echoSuppressed Audio Gate

Remove the `echoSuppressed` flag from `coordinator.ts`. AEC-cleaned audio flows to STT continuously in Rust. The VAD node's hybrid gate (0.85 threshold during TTS) handles echo suppression at the audio level.

### 2c. Flush Cascade Protocol

On barge-in, the Rust orchestrator executes:

```
1. tts_handle.flush(request_id)       → TTS cancels synthesis, acks
2. output_handle.flush(request_id)    → Output drops buffer, acks
3. Wait for both acks
4. stt_handle.barge_in()              → STT restarts cloud session
5. vad_handle.reset()                 → VAD clears frame counter
6. aec_handle.reset_buffers()         → AEC clears echo model
7. Emit Event::SpeakDone { request_id, reason: "interrupted" }
```

### 2d. Request ID Lifecycle

```
TS sends Speak(text, requestId="req-001")
  → Rust TTS node tags all audio with requestId="req-001"
  → Rust Output node accepts only chunks matching active requestId
  → On barge-in: flush(requestId="req-001"), output rejects late chunks
  → New Speak(text, requestId="req-002") starts clean
```

### 2e. Sentence-Boundary Chunking

In TypeScript coordinator, between LLM streaming and TTS:

```typescript
for (const char of delta) {
  sentenceBuffer += char;
  if (isSentenceEnd(char)) {  // 。！？.!? and newlines
    audioOutput.speakChunk(sentenceBuffer, requestId);
    sentenceBuffer = "";
  }
}
```

---

## Section 3: IPC Protocol Changes

### Commands (TS → Rust)

```typescript
// Existing commands gain requestId:
{ cmd: "speak", text: "...", requestId: "req-001", ...cloudConfig }
{ cmd: "speak_start", requestId: "req-001" }
{ cmd: "speak_chunk", text: "...", requestId: "req-001" }
{ cmd: "speak_end", requestId: "req-001" }
{ cmd: "stop_speaking" }  // flush-all

// New:
{ cmd: "flush_speak", requestId: "req-001" }
```

### Events (Rust → TS)

```typescript
{ event: "speak_started", requestId: "req-001" }
{ event: "speak_done", requestId: "req-001", reason: "completed" | "interrupted" | "error" }
{ event: "flush_ack", requestId: "req-001" }
```

---

## Section 4: TypeScript Layer Simplification

### Remove false abstractions

- Remove `STTProvider` interface and `RustWhisperSTT` (feed() is no-op)
- Remove `AudioSource.onAudio → sttProvider.feed` wiring (audio flows in Rust)
- Remove `TTSProvider` interface (TTS is Rust-internal)

### Simplified AudioOutput interface

```typescript
export interface AudioOutput {
  speak(text: string, requestId: string): void;
  speakStart(requestId: string): void;
  speakChunk(text: string, requestId: string): void;
  speakEnd(requestId: string): void;
  stop(): void;
  flush(requestId: string): void;
  onDone(cb: (requestId: string, reason: string) => void): void;
}
```

### Coordinator responsibilities (post-refactor)

1. **Turn detection** — VAD + transcript events → SegmentationEngine → LLM call
2. **LLM orchestration** — Stream LLM response, handle tool calls
3. **Sentence chunking** — Break LLM deltas into sentences → TTS
4. **Barge-in** — On barge-in event from Rust, cancel LLM stream + send stop_speaking

### Gateway simplification

- IPC events go directly to coordinator (no intermediary wrappers)
- Coordinator talks to audioOutput (IPC wrapper) and segmentationEngine only

---

## Files Modified

### Rust layer
- `pipeline/mod.rs` — New types: NodeId, RequestId, FlushSignal, FlushAck, PipelineNode trait, revised OutputMessage
- `pipeline/output.rs` — Request tracking, ring buffer flush, chunk rejection
- `pipeline/tts.rs` — Request ID threading, flush handler with ack
- `pipeline/vad.rs` — Implement PipelineNode trait
- `pipeline/stt.rs` — Implement PipelineNode trait
- `pipeline/capture.rs` — Implement PipelineNode trait
- `pipeline/aec.rs` — Implement PipelineNode trait
- `main.rs` — Flush cascade orchestration, request_id generation/routing, IPC protocol changes
- `output.rs` (root) — StreamingOutput clearing support

### TypeScript layer
- `src/ipc/protocol.ts` — Add requestId to commands/events, new flush_speak command
- `src/pipeline/interfaces.ts` — Remove STTProvider/TTSProvider, revise AudioOutput
- `src/pipeline/coordinator.ts` — Remove echoSuppressed, add sentence chunking, simplify wiring
- `src/pipeline/output/rust-playback.ts` — Implement revised AudioOutput with requestId
- `src/pipeline/sources/rust-capture.ts` — Simplify (remove STT-related methods)
- `src/pipeline/stt/rust-whisper.ts` — Delete (false abstraction)
- `src/channel/gateway.ts` — Simplify wiring, remove intermediary objects
