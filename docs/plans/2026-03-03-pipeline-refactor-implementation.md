# Pipeline Refactor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Refactor the audio pipeline to fix overlapping audio and wrong STT during interruption, adding request tracking, flush cascade, PipelineNode trait, sentence chunking, and TypeScript simplification.

**Architecture:** Rust owns the audio pipeline; TypeScript is a thin IPC orchestrator. The PipelineNode trait enables future graph-based orchestration. RequestId tracking prevents stale audio from leaking across sessions.

**Tech Stack:** Rust (tokio, serde, async-trait), TypeScript (vitest), IPC via JSON over stdin/stdout

---

## Phase 1: Rust Foundation Types

### Task 1: Add NodeId, RequestId, FlushSignal, FlushAck types

**Files:**
- Modify: `native/noisy-claw-audio/src/pipeline/mod.rs`

**Step 1: Write tests for the new types**

Add at the bottom of `native/noisy-claw-audio/src/pipeline/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_eq_and_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(NodeId::Capture);
        set.insert(NodeId::Tts);
        assert!(set.contains(&NodeId::Capture));
        assert!(!set.contains(&NodeId::Vad));
    }

    #[test]
    fn request_id_clone_and_eq() {
        let id1 = RequestId("req-001".to_string());
        let id2 = id1.clone();
        assert_eq!(id1, id2);
    }

    #[test]
    fn flush_ack_carries_node_id() {
        let ack = FlushAck {
            node: NodeId::Output,
            request_id: Some("req-001".to_string()),
        };
        assert_eq!(ack.node, NodeId::Output);
        assert_eq!(ack.request_id, Some("req-001".to_string()));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cd native/noisy-claw-audio && cargo test --lib pipeline::tests`
Expected: FAIL — `NodeId`, `RequestId`, `FlushAck` not defined

**Step 3: Add the type definitions**

Add after the existing `use` items in `native/noisy-claw-audio/src/pipeline/mod.rs`, before `AudioFrame`:

```rust
/// Identifies a node in the pipeline for flush acknowledgment.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeId {
    Capture,
    Aec,
    Vad,
    Stt,
    Tts,
    Output,
}

/// Opaque request identifier for tracking audio through the pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub String);

/// Signal to flush buffered data for a specific request or all requests.
pub enum FlushSignal {
    Flush { request_id: String },
    FlushAll,
}

/// Acknowledgment that a node has completed flushing.
pub struct FlushAck {
    pub node: NodeId,
    pub request_id: Option<String>,
}
```

**Step 4: Run tests to verify they pass**

Run: `cd native/noisy-claw-audio && cargo test --lib pipeline::tests`
Expected: PASS

**Step 5: Commit**

```bash
git add native/noisy-claw-audio/src/pipeline/mod.rs
git commit -m "feat(pipeline): add NodeId, RequestId, FlushSignal, FlushAck types"
```

---

### Task 2: Add PipelineNode trait

**Files:**
- Modify: `native/noisy-claw-audio/Cargo.toml` (add `async-trait`)
- Modify: `native/noisy-claw-audio/src/pipeline/mod.rs`

**Step 1: Add async-trait dependency**

In `native/noisy-claw-audio/Cargo.toml`, add under `[dependencies]`:

```toml
async-trait = "0.1"
```

**Step 2: Add the PipelineNode trait**

Add after `FlushAck` in `native/noisy-claw-audio/src/pipeline/mod.rs`:

```rust
/// Common trait for all pipeline nodes.
///
/// Domain-specific commands remain in per-node `Control` enums.
/// This trait enables future graph-based orchestration and provides
/// a uniform flush/shutdown protocol.
#[async_trait::async_trait]
pub trait PipelineNode: Send + 'static {
    fn node_id(&self) -> NodeId;
    async fn flush(&mut self, signal: FlushSignal) -> FlushAck;
    async fn shutdown(&mut self);
}
```

**Step 3: Verify it compiles**

Run: `cd native/noisy-claw-audio && cargo check`
Expected: OK (trait is defined but not yet implemented)

**Step 4: Commit**

```bash
git add native/noisy-claw-audio/Cargo.toml native/noisy-claw-audio/src/pipeline/mod.rs
git commit -m "feat(pipeline): add PipelineNode trait with async-trait"
```

---

### Task 3: Revise OutputMessage with RequestId and sample_rate

**Files:**
- Modify: `native/noisy-claw-audio/src/pipeline/mod.rs`

**Step 1: Write test for revised OutputMessage**

Add to the existing `#[cfg(test)] mod tests` in `pipeline/mod.rs`:

```rust
#[test]
fn output_message_audio_chunk_carries_metadata() {
    let msg = OutputMessage::AudioChunk {
        request_id: RequestId("req-001".to_string()),
        samples: vec![0.1, 0.2],
        sample_rate: 16000,
    };
    match msg {
        OutputMessage::AudioChunk { request_id, samples, sample_rate } => {
            assert_eq!(request_id, RequestId("req-001".to_string()));
            assert_eq!(samples.len(), 2);
            assert_eq!(sample_rate, 16000);
        }
        _ => panic!("expected AudioChunk"),
    }
}

#[test]
fn output_message_start_session_carries_request_id() {
    let msg = OutputMessage::StartSession {
        request_id: RequestId("req-002".to_string()),
        sample_rate: 24000,
    };
    match msg {
        OutputMessage::StartSession { request_id, sample_rate } => {
            assert_eq!(request_id, RequestId("req-002".to_string()));
            assert_eq!(sample_rate, 24000);
        }
        _ => panic!("expected StartSession"),
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cd native/noisy-claw-audio && cargo test --lib pipeline::tests`
Expected: FAIL — OutputMessage variants don't have request_id

**Step 3: Update OutputMessage enum**

Replace the existing `OutputMessage` enum in `pipeline/mod.rs`:

```rust
/// Messages sent to the output node (from TTS node and orchestrator).
pub enum OutputMessage {
    /// Begin a new playback session at the given sample rate.
    StartSession { request_id: RequestId, sample_rate: u32 },
    /// PCM audio chunk to write to the ring buffer.
    AudioChunk { request_id: RequestId, samples: Vec<f32>, sample_rate: u32 },
    /// All audio chunks have been sent; wait for buffer drain.
    FinishSession { request_id: RequestId },
    /// Stop a specific request immediately (interruption / barge-in).
    StopSession { request_id: RequestId },
    /// Stop all active sessions.
    StopAll,
}
```

**Step 4: Fix compilation errors in output.rs and tts.rs**

This change will break `output.rs` and `tts.rs` because they construct `OutputMessage` variants with the old signatures. We need to update them to compile, using a temporary placeholder `RequestId` until Task 6 threads real IDs through.

In `native/noisy-claw-audio/src/pipeline/tts.rs`, every place that constructs `OutputMessage` needs updating:

- `OutputMessage::StartSession { sample_rate }` → `OutputMessage::StartSession { request_id: RequestId("pending".to_string()), sample_rate }`
- `OutputMessage::AudioChunk(chunk)` → `OutputMessage::AudioChunk { request_id: RequestId("pending".to_string()), samples: chunk, sample_rate: 16000 }`
- `OutputMessage::FinishSession` → `OutputMessage::FinishSession { request_id: RequestId("pending".to_string()) }`
- `OutputMessage::StopSession` → `OutputMessage::StopSession { request_id: RequestId("pending".to_string()) }`

Add `use super::RequestId;` at the top of `tts.rs`.

In `native/noisy-claw-audio/src/pipeline/output.rs`, update pattern matches:

- `OutputMessage::StartSession { sample_rate }` → `OutputMessage::StartSession { request_id: _, sample_rate }`
- `OutputMessage::AudioChunk(samples)` → `OutputMessage::AudioChunk { request_id: _, samples, sample_rate: _ }`
- `OutputMessage::FinishSession` → `OutputMessage::FinishSession { request_id: _ }`
- `OutputMessage::StopSession` → `OutputMessage::StopSession { request_id: _ }`

Add a `StopAll` arm: `OutputMessage::StopAll => { /* same as StopSession for now */ }`

In `native/noisy-claw-audio/src/main.rs`, update:

- `pipeline::OutputMessage::StopSession` → `pipeline::OutputMessage::StopAll` (lines 174, 326)

**Step 5: Run tests to verify everything passes**

Run: `cd native/noisy-claw-audio && cargo test`
Expected: PASS

**Step 6: Commit**

```bash
git add native/noisy-claw-audio/src/pipeline/mod.rs native/noisy-claw-audio/src/pipeline/output.rs native/noisy-claw-audio/src/pipeline/tts.rs native/noisy-claw-audio/src/main.rs
git commit -m "feat(pipeline): add RequestId to OutputMessage variants"
```

---

## Phase 2: Rust IPC Protocol

### Task 4: Add requestId and reason to Rust IPC protocol

**Files:**
- Modify: `native/noisy-claw-audio/src/protocol.rs`

**Step 1: Write tests for new protocol fields**

Add to existing `mod tests` in `protocol.rs`:

```rust
#[test]
fn deserialize_speak_with_request_id() {
    let json = r#"{"cmd":"speak","text":"hello","tts":{"provider":"aliyun"},"request_id":"req-001"}"#;
    let cmd: Command = serde_json::from_str(json).unwrap();
    match cmd {
        Command::Speak { text, tts, request_id } => {
            assert_eq!(text, "hello");
            assert_eq!(request_id, Some("req-001".to_string()));
        }
        _ => panic!("expected Speak"),
    }
}

#[test]
fn deserialize_speak_without_request_id() {
    let json = r#"{"cmd":"speak","text":"hello","tts":{"provider":"aliyun"}}"#;
    let cmd: Command = serde_json::from_str(json).unwrap();
    match cmd {
        Command::Speak { request_id, .. } => {
            assert!(request_id.is_none());
        }
        _ => panic!("expected Speak"),
    }
}

#[test]
fn deserialize_flush_speak() {
    let json = r#"{"cmd":"flush_speak","request_id":"req-001"}"#;
    let cmd: Command = serde_json::from_str(json).unwrap();
    match cmd {
        Command::FlushSpeak { request_id } => {
            assert_eq!(request_id, "req-001");
        }
        _ => panic!("expected FlushSpeak"),
    }
}

#[test]
fn serialize_speak_done_with_reason() {
    let event = Event::SpeakDone {
        request_id: Some("req-001".to_string()),
        reason: "completed".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["event"], "speak_done");
    assert_eq!(v["request_id"], "req-001");
    assert_eq!(v["reason"], "completed");
}

#[test]
fn serialize_speak_started_with_request_id() {
    let event = Event::SpeakStarted {
        request_id: Some("req-001".to_string()),
    };
    let json = serde_json::to_string(&event).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["event"], "speak_started");
    assert_eq!(v["request_id"], "req-001");
}

#[test]
fn serialize_flush_ack() {
    let event = Event::FlushAck {
        request_id: "req-001".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["event"], "flush_ack");
    assert_eq!(v["request_id"], "req-001");
}
```

**Step 2: Run tests to verify they fail**

Run: `cd native/noisy-claw-audio && cargo test --lib protocol::tests`
Expected: FAIL

**Step 3: Update Command enum**

Add `request_id: Option<String>` to `Speak`, `SpeakStart`, `SpeakChunk`, `SpeakEnd`. Add new `FlushSpeak` variant:

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    StartCapture {
        #[serde(default = "default_device")]
        device: String,
        #[serde(default = "default_sample_rate")]
        sample_rate: u32,
        stt: Option<SttConfig>,
    },
    StopCapture,
    Speak {
        text: String,
        tts: TtsConfig,
        request_id: Option<String>,
    },
    SpeakStart {
        tts: TtsConfig,
        request_id: Option<String>,
    },
    SpeakChunk {
        text: String,
        request_id: Option<String>,
    },
    SpeakEnd {
        request_id: Option<String>,
    },
    StopSpeaking,
    FlushSpeak {
        request_id: String,
    },
    PlayAudio {
        path: String,
    },
    StopPlayback,
    GetStatus,
    Shutdown,
}
```

**Step 4: Update Event enum**

```rust
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
    SpeakStarted {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
    SpeakDone {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        reason: String,
    },
    FlushAck {
        request_id: String,
    },
    PlaybackDone,
    Status {
        capturing: bool,
        playing: bool,
        speaking: bool,
    },
    Error {
        message: String,
    },
}
```

**Step 5: Fix compilation errors in main.rs**

Every place in `main.rs` that constructs `Event::SpeakDone` or `Event::SpeakStarted` needs updating:

- `Event::SpeakDone` → `Event::SpeakDone { request_id: None, reason: "completed".to_string() }` (for normal completion)
- `Event::SpeakDone` (barge-in) → `Event::SpeakDone { request_id: None, reason: "interrupted".to_string() }`
- `Event::SpeakStarted` → `Event::SpeakStarted { request_id: None }`

In `main.rs` `handle_command`, update `Command::Speak`, `Command::SpeakStart`, `Command::SpeakChunk`, `Command::SpeakEnd` match patterns to destructure `request_id`. Add a `Command::FlushSpeak { .. }` arm (empty for now).

Also update the existing tests in `protocol.rs` that construct or assert on these events — particularly:
- `serialize_speak_started` — add `request_id: None`
- `serialize_speak_done` — add `request_id: None, reason: "completed".to_string()`
- `all_events_produce_valid_json_with_event_field` — update SpeakStarted and SpeakDone constructors
- `round_trip_all_commands` — add `{"cmd":"flush_speak","request_id":"req-001"}` to the list, and update `speak_end` to have empty body `{}`

**Step 6: Run all tests**

Run: `cd native/noisy-claw-audio && cargo test`
Expected: PASS

**Step 7: Commit**

```bash
git add native/noisy-claw-audio/src/protocol.rs native/noisy-claw-audio/src/main.rs
git commit -m "feat(ipc): add requestId to speak commands, reason to speak_done, flush_speak command"
```

---

## Phase 3: Rust Node Updates

### Task 5: Output node — request tracking and chunk rejection

**Files:**
- Modify: `native/noisy-claw-audio/src/pipeline/output.rs`

**Step 1: Add active_request_id tracking to output node**

Update `output.rs` to track the active `RequestId` and reject mismatched chunks:

```rust
use super::{AudioFrame, OutputMessage, OutputNodeEvent, RequestId, NodeId, FlushSignal, FlushAck};

// Inside the spawned task, after `let mut streaming_output`:
let mut active_request_id: Option<RequestId> = None;
```

**Step 2: Update StartSession handler to store request_id**

```rust
OutputMessage::StartSession { request_id, sample_rate } => {
    // Clean up previous session
    if let Some(ref mut out) = streaming_output {
        out.stop();
    }
    if let Some(h) = ref_fwd_handle.take() {
        h.abort();
    }

    active_request_id = Some(request_id.clone());
    tts_sample_rate = sample_rate;
    // ... rest of StreamingOutput::new() logic unchanged
```

**Step 3: Update AudioChunk handler to reject stale chunks**

```rust
OutputMessage::AudioChunk { request_id, samples, sample_rate: chunk_sr } => {
    if active_request_id.as_ref() != Some(&request_id) {
        tracing::debug!(
            ?request_id,
            ?active_request_id,
            "output node: rejecting stale audio chunk"
        );
        continue;
    }
    if let Some(ref mut out) = streaming_output {
        let written = out.write_samples(&samples, chunk_sr);
        tracing::debug!(
            chunk_samples = samples.len(),
            written,
            "output node: audio chunk written"
        );
    }
}
```

**Step 4: Update FinishSession to check request_id**

```rust
OutputMessage::FinishSession { request_id } => {
    if active_request_id.as_ref() != Some(&request_id) {
        tracing::debug!(?request_id, "output node: ignoring stale FinishSession");
        continue;
    }
    // ... existing drain logic ...
    active_request_id = None;
}
```

**Step 5: Update StopSession to clear request_id and drop StreamingOutput**

```rust
OutputMessage::StopSession { request_id } => {
    if active_request_id.as_ref() == Some(&request_id) || active_request_id.is_some() {
        if let Some(ref mut out) = streaming_output {
            out.stop();
        }
        streaming_output = None; // Drop ring buffer entirely
        if let Some(h) = ref_fwd_handle.take() {
            h.abort();
        }
        active_request_id = None;
        tracing::info!("output node: session stopped (interrupted)");
    }
}

OutputMessage::StopAll => {
    if let Some(ref mut out) = streaming_output {
        out.stop();
    }
    streaming_output = None;
    if let Some(h) = ref_fwd_handle.take() {
        h.abort();
    }
    active_request_id = None;
    tracing::info!("output node: all sessions stopped");
}
```

**Step 6: Verify compilation**

Run: `cd native/noisy-claw-audio && cargo check`
Expected: OK

**Step 7: Run existing tests**

Run: `cd native/noisy-claw-audio && cargo test`
Expected: PASS

**Step 8: Commit**

```bash
git add native/noisy-claw-audio/src/pipeline/output.rs
git commit -m "feat(output): add request_id tracking, reject stale chunks, drop ring buffer on stop"
```

---

### Task 6: TTS node — thread RequestId through all messages

**Files:**
- Modify: `native/noisy-claw-audio/src/pipeline/tts.rs`

**Step 1: Add request_id to TTS Control variants**

```rust
use super::{OutputMessage, RequestId};

pub enum Control {
    Speak { text: String, tts_config: TtsConfig, request_id: RequestId },
    SpeakStart { tts_config: TtsConfig, request_id: RequestId },
    SpeakChunk { text: String },
    SpeakEnd,
    Stop,
    Shutdown,
}
```

**Step 2: Update Handle methods to accept RequestId**

```rust
pub async fn speak(&self, text: String, tts_config: TtsConfig, request_id: RequestId) {
    let _ = self.control_tx.send(Control::Speak { text, tts_config, request_id }).await;
}

pub async fn speak_start(&self, tts_config: TtsConfig, request_id: RequestId) {
    let _ = self.control_tx.send(Control::SpeakStart { tts_config, request_id }).await;
}
```

**Step 3: Thread request_id through OutputMessage construction**

In `Control::Speak` handler — store `request_id` and pass to all `OutputMessage` constructors:

```rust
Control::Speak { text, tts_config, request_id } => {
    // ...
    let req_id = request_id.clone();
    let req_id2 = request_id.clone();
    synthesis_handle = Some(tokio::spawn(async move {
        let _ = out_tx.send(OutputMessage::StartSession {
            request_id: req_id.clone(),
            sample_rate,
        }).await;

        // In the forwarding task:
        let req_id_fwd = req_id.clone();
        let fwd = tokio::spawn(async move {
            while let Some(chunk) = chunk_rx.recv().await {
                let _ = out_tx2.send(OutputMessage::AudioChunk {
                    request_id: req_id_fwd.clone(),
                    samples: chunk,
                    sample_rate,
                }).await;
            }
        });

        // ... after forwarding:
        let _ = out_tx.send(OutputMessage::FinishSession {
            request_id: req_id2,
        }).await;
    }));
}
```

Apply the same pattern to `Control::SpeakStart` handler.

**Step 4: Store active_request_id for streaming chunks**

Add state: `let mut active_request_id: Option<RequestId> = None;`

In `SpeakStart`: `active_request_id = Some(request_id.clone());`
In forwarding task: use `active_request_id.clone()` for chunk tagging.
In `SpeakEnd` / `Stop`: `active_request_id = None;`

**Step 5: Fix compilation errors in main.rs**

In `handle_command`, update calls to `tts_handle.speak()` and `tts_handle.speak_start()` to pass `RequestId`:

```rust
Command::Speak { text, tts, request_id } => {
    let req_id = RequestId(request_id.unwrap_or_else(|| format!("req-{}", uuid_counter())));
    // ... existing state updates ...
    tts_handle.speak(text, tts, req_id).await;
}
```

For `uuid_counter`, add a simple atomic counter at the top of `main.rs`:

```rust
use std::sync::atomic::AtomicU64;
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);
fn next_request_id() -> String {
    let n = REQUEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("req-{n:06}")
}
```

**Step 6: Verify it compiles and passes tests**

Run: `cd native/noisy-claw-audio && cargo test`
Expected: PASS

**Step 7: Commit**

```bash
git add native/noisy-claw-audio/src/pipeline/tts.rs native/noisy-claw-audio/src/main.rs
git commit -m "feat(tts): thread RequestId through all OutputMessage construction"
```

---

### Task 7: Implement PipelineNode trait on output and TTS nodes

**Files:**
- Modify: `native/noisy-claw-audio/src/pipeline/output.rs`
- Modify: `native/noisy-claw-audio/src/pipeline/tts.rs`

The PipelineNode trait is implemented on the `Handle` structs. VAD, STT, AEC, and Capture nodes get minimal implementations (flush is a no-op or reset). Output and TTS have real flush implementations.

**Step 1: Add flush command to output node Control**

In `output.rs`:

```rust
use super::{AudioFrame, OutputMessage, OutputNodeEvent, RequestId, NodeId, FlushSignal, FlushAck};
use tokio::sync::oneshot;

pub enum Control {
    Flush { signal: FlushSignal, reply: oneshot::Sender<FlushAck> },
    Shutdown,
}
```

Handle the `Flush` variant in the output task loop:

```rust
Control::Flush { signal, reply } => {
    // Stop and clear
    if let Some(ref mut out) = streaming_output {
        out.stop();
    }
    streaming_output = None;
    if let Some(h) = ref_fwd_handle.take() {
        h.abort();
    }
    let req_id = match &signal {
        FlushSignal::Flush { request_id } => Some(request_id.clone()),
        FlushSignal::FlushAll => None,
    };
    active_request_id = None;
    let _ = reply.send(FlushAck { node: NodeId::Output, request_id: req_id });
    tracing::info!("output node: flushed");
}
```

Update `Handle`:

```rust
pub async fn flush(&self, signal: FlushSignal) -> FlushAck {
    let (tx, rx) = oneshot::channel();
    let _ = self.control_tx.send(Control::Flush { signal, reply: tx }).await;
    rx.await.unwrap_or(FlushAck { node: NodeId::Output, request_id: None })
}
```

**Step 2: Add flush command to TTS node Control**

In `tts.rs`, add to Control:

```rust
Flush { signal: FlushSignal, reply: oneshot::Sender<FlushAck> },
```

Handle:

```rust
Control::Flush { signal, reply } => {
    cancel_active(&mut synthesis_handle, &mut tts_session, &mut forwarding_handle).await;
    let req_id = match &signal {
        FlushSignal::Flush { request_id } => Some(request_id.clone()),
        FlushSignal::FlushAll => None,
    };
    active_request_id = None;
    let _ = reply.send(FlushAck { node: NodeId::Tts, request_id: req_id });
    tracing::info!("TTS node: flushed");
}
```

Update TTS `Handle`:

```rust
pub async fn flush(&self, signal: FlushSignal) -> FlushAck {
    let (tx, rx) = oneshot::channel();
    let _ = self.control_tx.send(Control::Flush { signal, reply: tx }).await;
    rx.await.unwrap_or(FlushAck { node: NodeId::Tts, request_id: None })
}
```

**Step 3: Verify compilation and tests**

Run: `cd native/noisy-claw-audio && cargo test`
Expected: PASS

**Step 4: Commit**

```bash
git add native/noisy-claw-audio/src/pipeline/output.rs native/noisy-claw-audio/src/pipeline/tts.rs
git commit -m "feat(pipeline): add flush protocol to output and TTS nodes"
```

---

## Phase 4: Rust Orchestrator

### Task 8: Flush cascade with ack in main.rs

**Files:**
- Modify: `native/noisy-claw-audio/src/main.rs`

**Step 1: Store active request_id in orchestrator state**

Add to orchestrator state section:

```rust
let mut active_request_id: Option<String> = None;
```

**Step 2: Implement flush cascade in barge-in handler**

Replace the barge-in handler (lines 167-187) with:

```rust
Some(()) = barge_in_rx.recv() => {
    if is_speaking_tts {
        let req_id = active_request_id.take().unwrap_or_default();
        tracing::info!(%req_id, "orchestrator: barge-in — starting flush cascade");

        // 1. Flush TTS (cancel synthesis)
        let tts_ack = tts_handle.flush(pipeline::FlushSignal::Flush {
            request_id: req_id.clone(),
        }).await;
        tracing::info!(?tts_ack.node, "orchestrator: TTS flush ack");

        // 2. Flush Output (drop ring buffer)
        let out_ack = output_handle.flush(pipeline::FlushSignal::Flush {
            request_id: req_id.clone(),
        }).await;
        tracing::info!(?out_ack.node, "orchestrator: Output flush ack");

        // 3. Reset pipeline state
        is_speaking_tts = false;
        tts_speaking_tx.send_replace(false);
        vad_handle.set_threshold(0.5).await;
        vad_handle.reset().await;
        aec_handle.reset_buffers().await;

        // 4. Restart cloud STT
        stt_handle.barge_in().await;

        // 5. Emit speak_done with reason
        let _ = event_tx.send(Event::SpeakDone {
            request_id: Some(req_id),
            reason: "interrupted".to_string(),
        }).await;
    }
}
```

**Step 3: Update SpeakDone handler (output drains naturally)**

Replace internal_rx handler (lines 190-203):

```rust
Some(internal_event) = internal_rx.recv() => {
    match internal_event {
        pipeline::OutputNodeEvent::SpeakDone => {
            tracing::info!("orchestrator: speak done (natural)");
            if is_speaking_tts {
                let req_id = active_request_id.take();
                is_speaking_tts = false;
                tts_speaking_tx.send_replace(false);
                vad_handle.set_threshold(0.5).await;
                vad_handle.reset().await;
                aec_handle.reset_buffers().await;
                let _ = event_tx.send(Event::SpeakDone {
                    request_id: req_id,
                    reason: "completed".to_string(),
                }).await;
            }
        }
    }
}
```

**Step 4: Update handle_command to track request_id and use flush**

In `Speak` and `SpeakStart` handlers, store the request_id:

```rust
Command::Speak { text, tts, request_id } => {
    let req_id = request_id.unwrap_or_else(|| next_request_id());
    *active_request_id_ref = Some(req_id.clone());
    *is_speaking_tts = true;
    tts_speaking_tx.send_replace(true);
    vad_handle.set_threshold(0.85).await;
    let _ = event_tx.send(Event::SpeakStarted {
        request_id: Some(req_id.clone()),
    }).await;
    tts_handle.speak(text, tts, pipeline::RequestId(req_id)).await;
}
```

Pass `active_request_id` as `&mut Option<String>` to `handle_command`.

In `StopSpeaking` handler, use flush cascade instead of direct stop:

```rust
Command::StopSpeaking => {
    if *is_speaking_tts {
        let req_id = is_speaking_tts_ref.take().unwrap_or_default();
        let _ = tts_handle.flush(pipeline::FlushSignal::FlushAll).await;
        let _ = output_handle.flush(pipeline::FlushSignal::FlushAll).await;
        *is_speaking_tts = false;
        tts_speaking_tx.send_replace(false);
        vad_handle.set_threshold(0.5).await;
        vad_handle.reset().await;
        aec_handle.reset_buffers().await;
        let _ = event_tx.send(Event::SpeakDone {
            request_id: active_request_id_ref.take(),
            reason: "interrupted".to_string(),
        }).await;
    }
    // Also stop file-based playback
    if let Some(ref mut pb) = playback_engine {
        pb.stop();
    }
    *playback_done_rx = None;
}
```

Add `FlushSpeak` handler:

```rust
Command::FlushSpeak { request_id } => {
    let _ = tts_handle.flush(pipeline::FlushSignal::Flush {
        request_id: request_id.clone(),
    }).await;
    let _ = output_handle.flush(pipeline::FlushSignal::Flush {
        request_id: request_id.clone(),
    }).await;
    let _ = event_tx.send(Event::FlushAck { request_id }).await;
}
```

Note: `handle_command` needs `output_handle` added to its parameter list (it currently only has `output_msg_tx`). Refactor: pass `&pipeline::output::Handle` directly instead of `output_msg_tx`.

**Step 5: Verify compilation and tests**

Run: `cd native/noisy-claw-audio && cargo test`
Expected: PASS

**Step 6: Commit**

```bash
git add native/noisy-claw-audio/src/main.rs
git commit -m "feat(orchestrator): implement flush cascade with ack on barge-in and stop"
```

---

## Phase 5: TypeScript IPC Protocol

### Task 9: Add requestId to TypeScript protocol types

**Files:**
- Modify: `src/ipc/protocol.ts`

**Step 1: Write test for new protocol fields**

Create or update test file. Since protocol.ts is type definitions + parse/serialize, test serialization:

```typescript
// In a test file or inline verification:
// SpeakCommand now has requestId
const cmd: SpeakCommand = { cmd: "speak", text: "hi", tts: { provider: "aliyun" }, requestId: "req-001" };
```

**Step 2: Update Command types**

```typescript
export type SpeakCommand = {
  cmd: "speak";
  text: string;
  tts: TtsConfig;
  request_id?: string;
};

export type SpeakStartCommand = {
  cmd: "speak_start";
  tts: TtsConfig;
  request_id?: string;
};

export type SpeakChunkCommand = {
  cmd: "speak_chunk";
  text: string;
  request_id?: string;
};

export type SpeakEndCommand = {
  cmd: "speak_end";
  request_id?: string;
};

export type FlushSpeakCommand = {
  cmd: "flush_speak";
  request_id: string;
};
```

Add `FlushSpeakCommand` to the `Command` union.

**Step 3: Update Event types**

```typescript
export type SpeakStartedEvent = {
  event: "speak_started";
  request_id?: string;
};

export type SpeakDoneEvent = {
  event: "speak_done";
  request_id?: string;
  reason?: string;
};

export type FlushAckEvent = {
  event: "flush_ack";
  request_id: string;
};
```

Add `FlushAckEvent` to the `AudioEvent` union.

**Step 4: Verify TypeScript compiles**

Run: `npx tsc --noEmit`
Expected: OK (or fix any type errors in downstream files)

**Step 5: Commit**

```bash
git add src/ipc/protocol.ts
git commit -m "feat(ipc): add requestId to TS speak commands, reason to speak_done, flush types"
```

---

## Phase 6: TypeScript Simplification

### Task 10: Remove false abstractions from interfaces.ts

**Files:**
- Modify: `src/pipeline/interfaces.ts`

**Step 1: Remove STTProvider and TTSProvider interfaces**

Delete the `STTProvider` interface (lines 46-51) and `TTSProvider` interface (lines 60-62).

**Step 2: Revise AudioOutput interface**

Replace existing `AudioOutput` with:

```typescript
export interface AudioOutput {
  speak(text: string, requestId: string): void;
  speakStart(requestId: string): void;
  speakChunk(text: string, requestId: string): void;
  speakEnd(requestId: string): void;
  stop(): void;
  flush(requestId: string): void;
  isPlaying(): boolean;
  onDone(cb: (requestId: string, reason: string) => void): void;
}
```

**Step 3: Fix downstream compilation**

This will break `coordinator.ts`, `rust-playback.ts`, and `gateway.ts`. Fix them in subsequent tasks.

**Step 4: Commit**

```bash
git add src/pipeline/interfaces.ts
git commit -m "refactor(interfaces): remove STTProvider/TTSProvider, revise AudioOutput with requestId"
```

---

### Task 11: Update rust-playback.ts with requestId

**Files:**
- Modify: `src/pipeline/output/rust-playback.ts`

**Step 1: Implement revised AudioOutput interface**

Rewrite `RustLocalPlayback` to match the new `AudioOutput` interface:

```typescript
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

  speakChunk(text: string, requestId: string): void {
    this.subprocess.trySend({
      cmd: "speak_chunk",
      text,
      request_id: requestId,
    });
  }

  speakEnd(requestId: string): void {
    this.subprocess.trySend({
      cmd: "speak_end",
      request_id: requestId,
    });
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
      const reason = event.reason ?? "completed";
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
```

**Step 2: Verify TypeScript compiles**

Run: `npx tsc --noEmit`
Expected: Will have errors in coordinator.ts and gateway.ts — fix in next tasks

**Step 3: Commit**

```bash
git add src/pipeline/output/rust-playback.ts
git commit -m "refactor(playback): implement revised AudioOutput with requestId and flush"
```

---

### Task 12: Remove echoSuppressed from coordinator, add sentence chunking

**Files:**
- Modify: `src/pipeline/coordinator.ts`
- Modify: `src/pipeline/coordinator.test.ts`

**Step 1: Rewrite PipelineCoordinator**

Key changes:
1. Remove `echoSuppressed` state entirely
2. Remove `sttProvider` from components (audio flows in Rust)
3. Remove `audioSource.onAudio → sttProvider.feed` wiring
4. Simplify barge-in: VAD events go to segmentation OR barge-in (no TS-side timer — Rust handles barge-in)
5. Add sentence chunking for `speakStreaming()`
6. Use `requestId` on all speak calls

```typescript
import type { AudioSource, SegmentationEngine, AudioOutput, SegmentMetadata } from "./interfaces.js";

export type PipelineConfig = {
  audio: { device: string; sampleRate: number };
  stt: { model: string; language: string };
  sttConfig?: unknown;
};

export type PipelineComponents = {
  audioSource: AudioSource;
  segmentation: SegmentationEngine;
  audioOutput: AudioOutput;
};

let requestCounter = 0;
function nextRequestId(): string {
  return `req-${String(++requestCounter).padStart(6, "0")}`;
}

function isSentenceEnd(char: string): boolean {
  return "。！？.!?\n".includes(char);
}

export class PipelineCoordinator {
  private readonly components: PipelineComponents;
  private messageCallbacks: Array<(message: string, metadata: SegmentMetadata) => void> = [];
  private active = false;
  private paused = false;
  private currentConfig: PipelineConfig | null = null;

  constructor(components: PipelineComponents) {
    this.components = components;
    this.wireComponents();
  }

  private wireComponents(): void {
    const { audioSource, segmentation, audioOutput } = this.components;

    // VAD events → segmentation engine (turn detection)
    // Barge-in is handled entirely in Rust via the flush cascade.
    audioSource.onVAD((speaking) => {
      segmentation.onVAD(speaking);
    });

    // Segmentation messages → callbacks
    segmentation.onMessage((message, metadata) => {
      for (const cb of this.messageCallbacks) {
        cb(message, metadata);
      }
    });

    // AudioOutput done → no-op (state tracked by requestId)
    audioOutput.onDone((requestId, reason) => {
      console.log(`[noisy-claw] speak done: requestId=${requestId} reason=${reason}`);
    });
  }

  start(config: PipelineConfig): void {
    if (this.active) return;
    this.active = true;
    this.paused = false;
    this.currentConfig = config;
    const source = this.components.audioSource as { setSttConfig?: (c: unknown) => void };
    if (typeof source.setSttConfig === "function") {
      source.setSttConfig(config.sttConfig);
    }
    this.components.audioSource.start(config.audio);
  }

  stop(): void {
    if (!this.active) return;
    this.active = false;
    this.paused = false;
    this.components.audioSource.stop();
    this.components.segmentation.flush();
  }

  pause(): void {
    if (!this.active || this.paused) return;
    this.paused = true;
    this.components.audioSource.stop();
    this.components.segmentation.flush();
  }

  resume(): void {
    if (!this.active || !this.paused || !this.currentConfig) return;
    this.paused = false;
    this.components.audioSource.start(this.currentConfig.audio);
  }

  get isPaused(): boolean {
    return this.paused;
  }

  /** Speak full text in batch mode. */
  speak(text: string): string {
    const requestId = nextRequestId();
    this.components.audioOutput.speak(text, requestId);
    return requestId;
  }

  /** Start a streaming TTS session with sentence chunking. */
  speakStart(): string {
    const requestId = nextRequestId();
    this.components.audioOutput.speakStart(requestId);
    return requestId;
  }

  /** Feed a text delta. Buffers and sends at sentence boundaries. */
  private sentenceBuffer = "";
  speakChunk(text: string, requestId: string): void {
    for (const char of text) {
      this.sentenceBuffer += char;
      if (isSentenceEnd(char) && this.sentenceBuffer.trim().length > 0) {
        this.components.audioOutput.speakChunk(this.sentenceBuffer, requestId);
        this.sentenceBuffer = "";
      }
    }
  }

  /** End streaming session. Flushes any remaining sentence buffer. */
  speakEnd(requestId: string): void {
    if (this.sentenceBuffer.trim().length > 0) {
      this.components.audioOutput.speakChunk(this.sentenceBuffer, requestId);
      this.sentenceBuffer = "";
    }
    this.components.audioOutput.speakEnd(requestId);
  }

  /** Stop all audio output immediately. */
  stopSpeaking(): void {
    this.sentenceBuffer = "";
    this.components.audioOutput.stop();
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

**Step 2: Update coordinator.test.ts**

Update the test file to match the new interface. Remove `sttProvider` from mock components. Update speak calls to use requestId. Remove echoSuppressed-related tests. Add sentence chunking tests.

**Step 3: Run tests**

Run: `npx vitest run src/pipeline/coordinator.test.ts`
Expected: PASS

**Step 4: Commit**

```bash
git add src/pipeline/coordinator.ts src/pipeline/coordinator.test.ts
git commit -m "refactor(coordinator): remove echoSuppressed, add sentence chunking, simplify to requestId flow"
```

---

### Task 13: Simplify gateway and delete rust-whisper.ts

**Files:**
- Modify: `src/channel/gateway.ts`
- Delete: `src/pipeline/stt/rust-whisper.ts`
- Modify: `src/pipeline/sources/rust-capture.ts` (remove unused `onAudio` callback handling if transcript events are no longer routed here)

**Step 1: Update gateway.ts to remove RustWhisperSTT**

Remove the import and usage of `RustWhisperSTT`. The `components` object no longer needs `sttProvider`. Transcript events from IPC go directly to the segmentation engine (or stay on the Rust side — transcripts are already emitted as IPC events and handled by `audioSource.onTranscript`).

Actually, looking at the flow: Rust emits `transcript` events via IPC → gateway routes them. The `RustWhisperSTT` was a pass-through that converted IPC events to `TranscriptSegment` and called callbacks. We need to keep transcript routing but simplify it.

In gateway.ts, replace the `rustSTT` usage with direct routing of transcript events to segmentation:

```typescript
// Remove: import { RustWhisperSTT } from "../pipeline/stt/rust-whisper.js";
// Remove: let rustSTT: RustWhisperSTT;
// Remove: rustSTT = new RustWhisperSTT();

// In onEvent handler:
if (event.event === "transcript") {
  const segment = {
    text: event.text,
    isFinal: event.is_final,
    start: event.start,
    end: event.end,
    confidence: event.confidence,
  };
  segmentation.onTranscript(segment);
}
```

Update `PipelineComponents` usage to match new shape (no `sttProvider`).

**Step 2: Delete rust-whisper.ts**

```bash
rm src/pipeline/stt/rust-whisper.ts
```

**Step 3: Update gateway speak calls to use requestId**

In the dispatch handler, update speak calls to use the new coordinator API:

```typescript
// Where pipeline.speakStart() was called:
const requestId = pipeline.speakStart();
// Where pipeline.speakChunk(text) was called:
pipeline.speakChunk(text, requestId);
// Where pipeline.speakEnd() was called:
pipeline.speakEnd(requestId);
```

**Step 4: Verify TypeScript compiles**

Run: `npx tsc --noEmit`
Expected: OK

**Step 5: Run all tests**

Run: `npx vitest run`
Expected: PASS

**Step 6: Commit**

```bash
git add src/channel/gateway.ts src/pipeline/sources/rust-capture.ts
git rm src/pipeline/stt/rust-whisper.ts
git commit -m "refactor(gateway): remove RustWhisperSTT, route transcripts directly, use requestId flow"
```

---

### Task 14: Final integration verification

**Step 1: Run all Rust tests**

Run: `cd native/noisy-claw-audio && cargo test`
Expected: PASS

**Step 2: Run all TypeScript tests**

Run: `npx vitest run`
Expected: PASS

**Step 3: Build Rust binary**

Run: `cd native/noisy-claw-audio && cargo build --release`
Expected: OK

**Step 4: Verify TypeScript compiles**

Run: `npx tsc --noEmit`
Expected: OK

**Step 5: Commit any remaining fixes**

```bash
git add -A
git commit -m "chore: final integration fixes for pipeline refactor"
```

---

## Phase 7: Thorough Code Review

### Task 15: Cross-layer consistency review

Review all changed files for consistency between the Rust IPC protocol and TypeScript protocol types.

**Step 1: Verify Rust ↔ TypeScript IPC parity**

Check that every field in `protocol.rs` `Command` enum is mirrored in `src/ipc/protocol.ts` `Command` union:
- `request_id` field names match (Rust uses `snake_case` serde, TS uses `snake_case` in IPC)
- `FlushSpeak` command exists in both
- `SpeakDone` event carries `request_id` and `reason` in both
- `FlushAck` event exists in both
- `SpeakStarted` event carries `request_id` in both

Run: `cd native/noisy-claw-audio && cargo test --lib protocol::tests`
Run: `npx tsc --noEmit`

**Step 2: Verify all `OutputMessage` construction sites use real RequestId**

Search for any remaining `"pending"` placeholder RequestIds left from Task 3:

Run: `rg "pending" native/noisy-claw-audio/src/pipeline/`
Expected: No results (all placeholders replaced in Task 6)

**Step 3: Verify no orphaned echoSuppressed references**

Run: `rg "echoSuppressed" src/`
Expected: No results

Run: `rg "echo_suppressed\|echoSuppressed" .`
Expected: No results in source files (may appear in plan docs)

**Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix: cross-layer consistency fixes from review"
```

---

### Task 16: Rust code quality review

Launch a **code-reviewer** agent to review all modified Rust files for:

**Checklist:**
- [ ] No `unwrap()` on channels that could be closed (use `unwrap_or` or handle `Err`)
- [ ] `FlushAck` oneshot channels have timeout protection (don't hang forever if node crashes)
- [ ] `RequestId` clone count is reasonable (not excessive cloning in hot paths)
- [ ] `OutputMessage::AudioChunk` comparison uses `PartialEq` correctly on `RequestId`
- [ ] `cancel_active()` in TTS properly aborts all handles before flush ack
- [ ] Barge-in flush cascade in `main.rs` awaits both acks before resetting pipeline
- [ ] No deadlock risk: flush uses oneshot (not mpsc) so node can't block waiting for reply
- [ ] `StopAll` in output node clears ring buffer (same as `StopSession`)
- [ ] `next_request_id()` atomic counter won't overflow in practice
- [ ] All `tracing::info!` calls use structured fields (no string interpolation)

**Files to review:**
- `native/noisy-claw-audio/src/pipeline/mod.rs`
- `native/noisy-claw-audio/src/pipeline/output.rs`
- `native/noisy-claw-audio/src/pipeline/tts.rs`
- `native/noisy-claw-audio/src/main.rs`
- `native/noisy-claw-audio/src/protocol.rs`

Run: `cd native/noisy-claw-audio && cargo clippy -- -W clippy::all`
Expected: No warnings

**Step 2: Fix any issues found**

**Step 3: Run full test suite**

Run: `cd native/noisy-claw-audio && cargo test`
Expected: PASS

**Step 4: Commit fixes**

```bash
git add native/noisy-claw-audio/
git commit -m "fix(rust): address code review findings"
```

---

### Task 17: TypeScript code quality review

Launch a **code-reviewer** agent to review all modified TypeScript files for:

**Checklist:**
- [ ] `PipelineCoordinator` no longer references `STTProvider` or `echoSuppressed`
- [ ] `AudioOutput` interface methods all pass `requestId` where needed
- [ ] `RustLocalPlayback.handleEvent` correctly handles missing `request_id` on events (backward compat)
- [ ] Sentence chunking handles edge cases: empty strings, whitespace-only, very long sentences
- [ ] `sentenceBuffer` is cleared on `stopSpeaking()` (no stale text leaks)
- [ ] `gateway.ts` routes transcript events directly to segmentation (no dead RustWhisperSTT code)
- [ ] `gateway.ts` speak calls use `requestId` from coordinator (not hardcoded)
- [ ] No unused imports remain after removing RustWhisperSTT
- [ ] `isSentenceEnd()` handles all CJK and Latin punctuation
- [ ] Module-level `requestCounter` is safe for single-threaded Node.js use

**Files to review:**
- `src/pipeline/interfaces.ts`
- `src/pipeline/coordinator.ts`
- `src/pipeline/coordinator.test.ts`
- `src/pipeline/output/rust-playback.ts`
- `src/channel/gateway.ts`
- `src/ipc/protocol.ts`

Run: `npx tsc --noEmit`
Expected: OK

**Step 2: Fix any issues found**

**Step 3: Run full test suite**

Run: `npx vitest run`
Expected: PASS

**Step 4: Commit fixes**

```bash
git add src/
git commit -m "fix(ts): address code review findings"
```

---

### Task 18: Integration smoke test

**Step 1: Build everything from clean state**

```bash
cd native/noisy-claw-audio && cargo build --release
cd ../.. && npx tsc --noEmit
```

Expected: Both pass

**Step 2: Run all Rust tests**

Run: `cd native/noisy-claw-audio && cargo test`
Expected: PASS

**Step 3: Run all TypeScript tests**

Run: `npx vitest run`
Expected: PASS

**Step 4: Run clippy with all warnings**

Run: `cd native/noisy-claw-audio && cargo clippy -- -W clippy::all -W clippy::pedantic -A clippy::module_name_repetitions -A clippy::too_many_arguments -A clippy::cast_possible_truncation`
Expected: No warnings (or only acceptable ones)

**Step 5: Verify no debug artifacts**

Run: `rg "TODO\|FIXME\|HACK\|XXX" native/noisy-claw-audio/src/ src/ --glob '!*.md'`
Expected: No unexpected results

Run: `rg "console\.log" src/ --glob '!*.test.*' --glob '!*.md'`
Review: Only intentional logging remains (prefixed with `[noisy-claw]`)

**Step 6: Final commit if needed**

```bash
git add -A
git commit -m "chore: final integration verification — all tests pass"
```

---

## Summary of Changes

| File | Change |
|------|--------|
| `pipeline/mod.rs` | +NodeId, +RequestId, +FlushSignal, +FlushAck, +PipelineNode trait, revised OutputMessage |
| `pipeline/output.rs` | +request_id tracking, +chunk rejection, +flush protocol, drop ring buffer on stop |
| `pipeline/tts.rs` | +request_id threading, +flush protocol |
| `protocol.rs` | +requestId on speak commands, +reason on speak_done, +flush_speak, +flush_ack |
| `main.rs` | +flush cascade, +request_id routing, +FlushSpeak handler |
| `src/ipc/protocol.ts` | +requestId fields, +flush types |
| `src/pipeline/interfaces.ts` | -STTProvider, -TTSProvider, revised AudioOutput |
| `src/pipeline/coordinator.ts` | -echoSuppressed, +sentence chunking, simplified wiring |
| `src/pipeline/output/rust-playback.ts` | Revised to match new AudioOutput |
| `src/channel/gateway.ts` | -RustWhisperSTT, direct transcript routing |
| `src/pipeline/stt/rust-whisper.ts` | DELETED |
