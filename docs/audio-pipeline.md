# Audio Pipeline Data Flow

## Architecture Overview

Two-process architecture:
- **Rust subprocess** (`noisy-claw-audio`): Real-time audio pipeline — capture, AEC, VAD, STT, TTS, output
- **TypeScript process** (OpenClaw plugin): Segmentation, dispatch, agent interaction, mode switching

The Rust pipeline runs at **48kHz** internally, downsampling to 16kHz for VAD (Silero) and STT providers.

---

## Rust Pipeline Node Graph (shared by all modes)

```
                                        ┌──────────────────┐
                                        │  render_ref_tx   │
                                        │  (speaker output │◄───┐
                                        │   reference)     │    │
                                        └────────┬─────────┘    │
                                                 │              │
  Microphone                                     ▼              │
     │                                     ┌─────┴──────┐      │
     ▼                                     │            │      │
  ┌─────────┐  capture_tx  ┌───────┐  cleaned_tx  ┌─────┐     │
  │ Capture │─────────────►│  AEC  │─────────────►│ VAD │     │
  │  Node   │  (48kHz)     │ Node  │  (48kHz,     │ Node│     │
  └─────────┘              └───────┘  echo-free)  └──┬──┘     │
                                                     │  │      │
                                       vad_audio_tx  │  │ vad_event_tx
                                       (48kHz+VAD)   │  │ (speaking)
                                                     ▼  ▼      │
                                                  ┌──────┐     │
                                                  │ STT  │     │
                                                  │ Node │     │
                                                  └──┬───┘     │
                                                     │         │
                                              event_tx (IPC)   │
                                             (transcripts)     │
                                                     │         │
           IPC cmd                                   ▼         │
           (speak/speak_start/                [stdout → TS]    │
            chunk/end)                                         │
                │                                              │
                ▼                                              │
            ┌──────┐  output_msg_tx  ┌────────┐  render_ref   │
            │ TTS  │────────────────►│ Output │────────────────┘
            │ Node │   (PCM audio)   │  Node  │
            └──────┘                 └────┬───┘
                                          │
                                          ▼
                                       Speaker
```

### Node Details

| Node | Rate | Function |
|------|------|----------|
| **Capture** | 48kHz | Opens mic via cpal, sends raw `AudioFrame` |
| **AEC** | 48kHz | Echo cancellation using speaker reference; convergence blanking (~400ms) on render start |
| **VAD** | 48k→16k | Silero VAD on 16kHz; forwards original 48kHz audio + VadState to STT; barge-in detection |
| **STT** | 48k→16k | Cloud: continuous feed; Local Whisper: VAD-gated accumulation. Emits `Event::Transcript` |
| **TTS** | varies | Batch (`speak`) or streaming (`speak_start/chunk/end`). Synthesizes PCM → Output |
| **Output** | device native | Plays PCM to speaker; feeds render reference back to AEC |

---

## TypeScript Pipeline (mode-dependent)

```
  Rust IPC Events                     TS Pipeline Coordinator
  ───────────────                     ──────────────────────

  Event::Vad ──────────────►  RustLocalCapture.onVAD
                                       │
                                       ▼
                              ┌─────────────────┐
                              │  Segmentation    │  ◄── mode-specific
                              │  Engine          │
                              │  .onVAD()        │
                              └────────┬─────────┘
                                       │
  Event::Transcript ──────►  RustWhisperSTT.onTranscript
                                       │
                                       ▼
                              ┌─────────────────┐
                              │  Segmentation    │
                              │  Engine          │
                              │  .onTranscript() │
                              └────────┬─────────┘
                                       │ emits when ready
                                       ▼
                              pipeline.onMessage()
                                       │
                                       ▼
                              dispatchVoiceTranscript()
                                       │
                                       ▼
                              OpenClaw Agent
                                       │
                              (agent may call voice_speak tool)
                                       │
                                       ▼
                              pipeline.speak() ──► Rust TTS → Output
```

---

## Mode 1: Conversation

**Segmentation:** `VADSilenceSegmentation` (silence threshold: 700ms)

```
  ┌─────────────────────────────────────────────────────────────────┐
  │                    RUST AUDIO PIPELINE                          │
  │                                                                 │
  │  Mic ──► Capture ──► AEC ──► VAD ──► STT                       │
  │              48kHz       48kHz   48k→16k  16kHz                 │
  │                           ▲                 │                   │
  │                    render ref          Event::Transcript        │
  │                           │                 │                   │
  │  Speaker ◄── Output ◄── TTS ◄── speak cmd  │                   │
  │                                     ▲       │                   │
  └─────────────────────────────────────┼───────┼───────────────────┘
                                        │       │  IPC (stdin/stdout)
  ┌─────────────────────────────────────┼───────┼───────────────────┐
  │                    TYPESCRIPT                │                   │
  │                                     │       ▼                   │
  │                              ┌──────────────────┐               │
  │                              │ VADSilence       │               │
  │                              │ Segmentation     │               │
  │                              │                  │               │
  │                              │ • emits on each  │               │
  │                              │   isFinal segment│               │
  │                              │ • 700ms silence  │               │
  │                              │   timer (turn)   │               │
  │                              └────────┬─────────┘               │
  │                                       │                         │
  │                                       ▼                         │
  │                          dispatchVoiceTranscript()               │
  │                          mode="conversation"                    │
  │                          prefix="[Voice input from microphone]" │
  │                          ttsEnabled=false                       │
  │                                       │                         │
  │                                       ▼                         │
  │                              ┌────────────────┐                 │
  │                              │ OpenClaw Agent │                 │
  │                              │                │                 │
  │                              │ Text response  │──► text only    │
  │                              │ (no auto-TTS)  │   (outbound    │
  │                              │                │    adapter)     │
  │                              │ OR calls       │                 │
  │                              │ voice_speak ───┼──► pipeline     │
  │                              │ tool           │   .speak(text)  │
  │                              └────────────────┘                 │
  └─────────────────────────────────────────────────────────────────┘
```

**Key behaviors:**
- Each `isFinal` transcript segment is dispatched **immediately**
- Agent responses are **text-only** by default
- Agent must explicitly call `voice_speak` tool to trigger TTS
- TTS uses **batch mode** (single `speak` command)

**Barge-in (during TTS playback):**
1. VAD threshold raised to 0.85 (from 0.5)
2. Requires 4 consecutive speech frames (~128ms) to confirm
3. Triggers flush cascade: TTS flush → Output flush → reset
4. Emits `SpeakDone { reason: "interrupted" }`

---

## Mode 2: Meeting

**Segmentation:** `MeetingSegmentation` (block accumulation + topic detection)

```
  ┌─────────────────────────────────────────────────────────────────┐
  │                    RUST AUDIO PIPELINE                          │
  │                                                                 │
  │  Mic ──► Capture ──► AEC ──► VAD ──► STT ─────┐                │
  │              48kHz       48kHz   48k→16k       │                │
  │                           ▲           Event::Transcript         │
  │                    render ref                  │                │
  │                           │        ┌───────────┤                │
  │                           │        │    (tap)  │                │
  │                           │        ▼           │                │
  │                           │  ┌──────────┐      │                │
  │                           │  │  Topic   │      │                │
  │                           │  │Detection │      │                │
  │                           │  │(MiniLM)  │      │                │
  │                           │  └────┬─────┘      │                │
  │                           │       │            │                │
  │                           │  Event::TopicShift │                │
  │                           │       │            │                │
  │  Speaker ◄── Output ◄── TTS      │            │                │
  │                  ▲                │            │                │
  └──────────────────┼────────────────┼────────────┼────────────────┘
                     │                │            │  IPC
  ┌──────────────────┼────────────────┼────────────┼────────────────┐
  │                  │  TYPESCRIPT    │            │                 │
  │                  │                ▼            ▼                 │
  │                  │       ┌────────────────────────┐             │
  │                  │       │ MeetingSegmentation    │             │
  │                  │       │                        │             │
  │                  │       │ Accumulates transcripts│             │
  │                  │       │ Emits block on:        │             │
  │                  │       │ • topic shift          │             │
  │                  │       │ • 30s silence          │             │
  │                  │       │ • 5min max duration    │             │
  │                  │       │ • 60s auto-stop        │             │
  │                  │       │                        │             │
  │                  │       │ Keyword detection:     │             │
  │                  │       │ "molty", "assistant"...│             │
  │                  │       └───┬──────────────┬─────┘             │
  │                  │           │              │                    │
  │                  │     normal block    keyword-addressed         │
  │                  │           │              │                    │
  │                  │           ▼              ▼                    │
  │                  │    dispatch()      dispatch()                 │
  │                  │    mode=           mode=                      │
  │                  │    "meeting"       "meeting-keyword"          │
  │                  │    tts=false       tts=true ◄── only auto-TTS│
  │                  │         │              │                      │
  │                  │         ▼              ▼                      │
  │                  │    Agent gets    Agent reply streamed         │
  │                  │    passive       sentence-by-sentence         │
  │                  │    transcript    to TTS (speakStart/          │
  │                  │    block         speakChunk/speakEnd)         │
  │                  │         │              │                      │
  │                  │    (can call      ┌────┘                      │
  │                  │    voice_speak)   │                           │
  │                  │         │         │                           │
  │                  └─────────┼─────────┘                          │
  │                            │                                    │
  │                     pipeline.speak*()                            │
  └─────────────────────────────────────────────────────────────────┘
```

**Key behaviors:**
- Transcripts **accumulate** into blocks (not emitted immediately)
- **Topic detection** (Rust): sentence embeddings via MiniLM, cosine similarity < 0.65 = shift
- **Keyword addressing**: transcript containing a keyword triggers **auto-TTS** streaming response
- Normal (non-keyword) blocks: text-only, agent receives passively
- Two TTS paths: batch (via `voice_speak`) and streaming (auto for keyword-addressed)

**Emission triggers:**

| Trigger | Condition | Result |
|---------|-----------|--------|
| Topic shift | Cosine similarity drops below 0.65 | Emit accumulated buffer |
| Silence block | 30s of continuous silence | Emit accumulated buffer |
| Max duration | 5min since first transcript in block | Force emit |
| Auto-stop | 60s of silence | Emit + fire auto-stop callbacks |
| Keyword | Transcript contains configured keyword | Immediate dispatch with auto-TTS |

---

## Mode 3: Dictation

**Segmentation:** `DictationSegmentation` (end-phrase detection)

```
  ┌─────────────────────────────────────────────────────────────────┐
  │                    RUST AUDIO PIPELINE                          │
  │                                                                 │
  │  Mic ──► Capture ──► AEC ──► VAD ──► STT                       │
  │              48kHz       48kHz   48k→16k  16kHz                 │
  │                           ▲                 │                   │
  │                    render ref          Event::Transcript        │
  │                           │                 │                   │
  │  Speaker ◄── Output ◄── TTS ◄── speak cmd  │                   │
  │                                     ▲       │                   │
  └─────────────────────────────────────┼───────┼───────────────────┘
                                        │       │  IPC
  ┌─────────────────────────────────────┼───────┼───────────────────┐
  │                    TYPESCRIPT        │       │                   │
  │                                     │       ▼                   │
  │                              ┌──────────────────┐               │
  │                              │ Dictation        │               │
  │                              │ Segmentation     │               │
  │                              │                  │               │
  │                              │ • VAD ignored    │               │
  │                              │ • accumulates    │               │
  │                              │   all isFinal    │               │
  │                              │   transcripts    │               │
  │                              │ • emits when     │               │
  │                              │   end phrase     │               │
  │                              │   detected:      │               │
  │                              │   "end dictation"│               │
  │                              │   "结束听写"      │               │
  │                              └────────┬─────────┘               │
  │                                       │                         │
  │           User says: "... end dictation"                        │
  │                                       │                         │
  │                                       ▼                         │
  │                          dispatchVoiceTranscript()               │
  │                          mode="dictation"                       │
  │                          prefix="[Dictation result]"            │
  │                          ttsEnabled=false                       │
  │                                       │                         │
  │                                       ▼                         │
  │                              ┌────────────────┐                 │
  │                              │ OpenClaw Agent │                 │
  │                              │                │                 │
  │                              │ Receives full  │                 │
  │                              │ dictated text  │──► text only    │
  │                              │ (end phrase    │                 │
  │                              │  stripped)     │                 │
  │                              │                │                 │
  │                              │ Can call       │                 │
  │                              │ voice_speak ───┼──► pipeline     │
  │                              │ tool           │   .speak(text)  │
  │                              └────────────────┘                 │
  └─────────────────────────────────────────────────────────────────┘
```

**Key behaviors:**
- **VAD is completely ignored** for segmentation (`onVAD` is a no-op)
- Transcripts accumulate **indefinitely** until an end phrase is detected
- End phrase is **stripped** from the emitted text
- No topic detection, no keyword addressing
- Agent receives the complete dictation result as a single block
- TTS only via explicit `voice_speak` tool calls

**Example flow:**
```
User: "Dear team comma I wanted to update you on the project period
       The deadline has been moved to next Friday period
       Please adjust your schedules accordingly period
       End dictation"

→ Accumulates: "Dear team, I wanted to update you on the project.
                The deadline has been moved to next Friday.
                Please adjust your schedules accordingly."

→ "end dictation" detected and stripped
→ Full text dispatched as "[Dictation result]: ..."
```

---

## Mode Comparison

| | Conversation | Meeting | Dictation |
|---|---|---|---|
| **Segmentation** | Immediate per segment | Accumulated blocks | End-phrase triggered |
| **VAD role** | 700ms silence timer | 30s silence block, 60s auto-stop | Ignored |
| **Topic detection** | No | Yes (MiniLM embeddings) | No |
| **Auto-TTS** | Never | Keyword-addressed only | Never |
| **TTS trigger** | `voice_speak` tool | Keyword: auto-stream; else `voice_speak` | `voice_speak` tool |
| **Dispatch prefix** | `[Voice input from microphone]` | `[Meeting transcript block]` | `[Dictation result]` |
| **Barge-in** | Yes (threshold 0.85, 4 frames) | Same | Same |
| **Rust-side difference** | Default | Spawns topic detection node | No special behavior |

---

## Voice Mode State Transitions

### Session Lifecycle

```
                          startAccount()
                               │
                               ▼
                    ┌─────────────────────┐
                    │                     │
                    │    INACTIVE         │
                    │    (no session)     │
                    │                     │
                    └──────────┬──────────┘
                               │ session.start()
                               │ pipeline.start()
                               │ subprocess.start()
                               ▼
                    ┌─────────────────────┐
                    │                     │◄─────────────────────┐
                    │    ACTIVE           │                      │
                    │    mode=conversation│  (initial mode)      │
                    │    listening=true   │                      │
                    │    speaking=false   │                      │
                    │                     │                      │
                    └──────────┬──────────┘                      │
                               │                                │
                     abort signal / stopAccount()               │
                               │                                │
                               ▼                                │
                    ┌─────────────────────┐                     │
                    │                     │                      │
                    │    STOPPED          │     (restart         │
                    │    subprocess.stop()│      requires new    │
                    │    pipeline.stop()  │      startAccount)   │
                    │    session.stop()   │─────────────────────►│
                    │                     │
                    └─────────────────────┘
```

### Mode Switching

Mode transitions are triggered by the AI agent calling the `voice_mode` tool,
or programmatically via `switchMode()`. The initial mode is always `conversation`.

**Conversation** is the "home" mode. **Meeting** and **dictation** are temporary
modes that automatically return to the previous mode (typically conversation)
when they complete:
- Meeting: auto-returns on prolonged silence (60s auto-stop)
- Dictation: auto-returns when end phrase is detected

```
                        ┌───────────────────────────────────────┐
                        │          voice_mode tool              │
                        │          or switchMode()              │
                        └───────────┬───────────────────────────┘
                                    │
                ┌───────────────────┼───────────────────┐
                │                   │                   │
                ▼                   ▼                   ▼
  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐
  │                  │  │                  │  │                  │
  │  CONVERSATION    │  │    MEETING       │  │   DICTATION      │
  │  (home mode)     │  │   (temporary)    │  │   (temporary)    │
  │                  │  │                  │  │                  │
  │  Segmentation:   │  │  Segmentation:   │  │  Segmentation:   │
  │  VADSilence      │  │  Meeting         │  │  Dictation       │
  │                  │  │                  │  │                  │
  │  Rust: default   │  │  Rust: topic     │  │  Rust: default   │
  │                  │  │  node spawned    │  │                  │
  └────────┬─────────┘  └────────┬─────────┘  └────────┬─────────┘
           │                     │                     │
           │   voice_mode tool   │                     │
           │────────────────────►│                     │
           │◄────────────────────│ auto-stop (60s)     │
           │◄────────────────────│ or voice_mode tool  │
           │                     │                     │
           │   voice_mode tool   │                     │
           │─────────────────────┼────────────────────►│
           │◄────────────────────┼─────────────────────│ end phrase
           │                     │◄────────────────────│ or voice_mode tool
           │                     │─────────────────────►│
           │                     │◄────────────────────│ end phrase
           │                     │                     │
```

**Auto-return behavior**: When entering meeting or dictation, the current mode
is saved as `previousMode`. On completion, `switchMode(previousMode)` is called
automatically. Manual mode switches via `voice_mode` tool are always allowed.

### What Happens on Each Transition

```
  ┌──────────────┐   switchMode(mode)   ┌──────────────┐
  │  Old Mode    │─────────────────────►│  New Mode    │
  └──────┬───────┘                      └──────┬───────┘
         │                                     │
         │  1. Create new SegmentationEngine   │
         │     based on target mode            │
         │                                     │
         │  2. coordinator.swapSegmentation()  │
         │     • old engine is dropped         │
         │     • new engine wired to VAD/STT   │
         │     • pending buffer is lost        │
         │                                     │
         │  3. Wire mode-specific callbacks    │
         │     • meeting: topicShift, keyword  │
         │     • others: no extra wiring       │
         │                                     │
         │  4. IPC: set_mode → Rust            │
         │     • meeting → spawn topic node    │
         │     • leaving meeting → kill topic  │
         │     • others → no Rust-side change  │
         │                                     │
         │  5. session.setMode(mode)           │
         │     • update session state          │
         │                                     │
```

### Rust-side Mode Effects

```
                     ┌──────────────────┐
                     │  set_mode cmd    │
                     │  received        │
                     └────────┬─────────┘
                              │
                     ┌────────┴─────────┐
                     │  mode=="meeting"?│
                     └────────┬─────────┘
                        yes   │    no
                    ┌─────────┴─────────────┐
                    ▼                       ▼
         ┌───────────────────┐   ┌───────────────────┐
         │ Spawn topic node  │   │ Kill topic node   │
         │ (if not running)  │   │ (if running)      │
         │                   │   │                   │
         │ • Load MiniLM     │   │ • topic_handle    │
         │   embeddings      │   │   .shutdown()     │
         │ • similarity=0.65 │   │                   │
         │ • max_block=300s  │   │                   │
         │ • silence=30s     │   │                   │
         └───────────────────┘   └───────────────────┘
                    │                       │
                    └───────────┬───────────┘
                                ▼
                     ┌───────────────────┐
                     │ current_mode =    │
                     │ mode              │
                     └───────────────────┘
```

### Speaking State (within any mode)

```
                    ┌──────────────────┐
                    │    LISTENING     │
                    │    (idle)        │
                    │                  │
                    │  VAD threshold:  │
                    │  0.5 (normal)    │
                    └────────┬─────────┘
                             │
              voice_speak tool / auto-TTS
              (meeting-keyword only)
                             │
                             ▼
                    ┌──────────────────┐
                    │    SPEAKING      │
                    │    (TTS active)  │
                    │                  │
                    │  VAD threshold:  │
                    │  0.85 (raised)   │
                    │                  │
                    │  Barge-in:       │
                    │  4 frames @0.85  │
                    │  (~128ms)        │
                    │                  │
                    │  Blanking:       │
                    │  • AEC warmup 3s │
                    │  • comfort 6frm  │
                    │  • post-flush 6f │
                    └───────┬──┬───────┘
                            │  │
               TTS complete │  │ barge-in detected
                            │  │
                            ▼  ▼
                    ┌──────────────────┐
                    │   FLUSH/DONE     │
                    │                  │
                    │  • TTS flushed   │
                    │  • Output flushed│
                    │  • VAD reset     │
                    │  • threshold→0.5 │
                    │                  │
                    │  SpeakDone:      │
                    │  "completed" or  │
                    │  "interrupted"   │
                    └────────┬─────────┘
                             │
                             ▼
                    ┌──────────────────┐
                    │    LISTENING     │
                    │    (idle)        │
                    └──────────────────┘
```

### Complete State Machine (combined)

```
  SESSION                          MODE                         SPEAKING
  ───────                          ────                         ────────

  ┌──────────┐                ┌──────────────┐              ┌───────────┐
  │          │  start()       │              │              │           │
  │ INACTIVE ├───────────────►│ CONVERSATION │◄────────────►│ LISTENING │
  │          │                │              │   voice_mode │           │
  └──────────┘                └──────┬───────┘   tool       └─────┬─────┘
       ▲                             │                            │
       │                    ┌────────┴────────┐           voice_speak /
       │                    ▼                 ▼           auto-TTS
       │             ┌──────────┐      ┌──────────┐           │
       │             │          │      │          │           ▼
       │  stop()     │ MEETING  │◄────►│ DICTATION│     ┌───────────┐
       │◄────────────│          │      │          │     │           │
       │   from any  └──────────┘      └──────────┘     │ SPEAKING  │
       │   mode                                         │           │
       │                                                └─────┬─────┘
       │                                                      │
       │                                          TTS done /  │
       │                                          barge-in    │
       │                                                      ▼
       │                                                ┌───────────┐
       │                                                │           │
       └────────────────────────────────────────────────│ LISTENING │
                                                        │           │
                                                        └───────────┘
```
