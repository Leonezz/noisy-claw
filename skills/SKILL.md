---
name: skills
description: Control the voice channel — speak, listen, switch modes, check status, manage transcripts
---

# Voice Channel

You have access to a voice channel that lets users speak to you and hear your responses. The system uses real-time speech-to-text, text-to-speech, and echo cancellation for natural conversation. Three operating modes are available: **conversation**, **meeting**, and **dictation**.

---

## Tools

### `voice_mode` — Switch operating mode

**Parameters:** `mode` (`"conversation"` | `"meeting"` | `"dictation"`)

Switches the voice channel to a different operating mode. The switch takes effect immediately — the current transcript buffer is flushed, the segmentation engine is replaced, and the Rust audio subprocess is notified.

| Mode | Purpose | TTS behavior |
|------|---------|--------------|
| `conversation` | Turn-based dialogue | Text-only by default — use `voice_speak` to respond aloud |
| `meeting` | Passive observation with topic-based segmentation | Text-only — use `voice_speak` for brief spoken feedback |
| `dictation` | Continuous capture until user says an end phrase | Text-only — avoid speaking during dictation |

**When to use:**
- Switch to `meeting` when the user asks you to listen in on a meeting, take notes, or observe a discussion without interrupting.
- Switch to `dictation` when the user wants to dictate a long block of text (email, document, note) without being interrupted by TTS responses.
- Switch back to `conversation` when the user wants normal back-and-forth dialogue.

**Example:**
```
// User says: "Switch to meeting mode, I have a call starting"
voice_mode({ mode: "meeting" })

// Later: "OK the meeting is over, let's talk"
voice_mode({ mode: "conversation" })
```

---

### `voice_speak` — Synthesize speech

**Parameters:** `text` (string)

Speaks the given text aloud through TTS. Only works when the voice pipeline is active. **This is the only way to produce voice output** — your text replies are never automatically spoken. You choose when to speak.

**When to use:**
- When the user is interacting via voice and expects a spoken reply — speak the key part of your response (1-3 sentences)
- Greetings, acknowledgements, or alerts
- Reading out a specific piece of information on demand
- Brief spoken feedback during meeting mode when keyword-addressed

**When NOT to use:**
- When your reply is long or contains code/lists/links — just reply with text and optionally say "check the text chat for details"
- During dictation mode — don't interrupt the user's dictation
- Don't speak your entire text reply — speak a concise summary and let the full text appear in chat

---

### `voice_listen` — Control microphone

**Parameters:** `listening` (boolean)

Starts (`true`) or pauses (`false`) microphone capture. When paused, no audio is captured and no transcription occurs.

**When to use:**
- Pause the mic when performing a long computation or tool call where you don't need user input
- Pause when the user asks to "hold on" or "wait"
- Resume when ready to receive speech input again

---

### `voice_status` — Query session state

**Parameters:** none

Returns a JSON object with the current voice channel state:

```json
{
  "active": true,
  "mode": "conversation",
  "duration": 45,
  "segmentCount": 12,
  "currentlyListening": true,
  "currentlySpeaking": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `active` | boolean | Whether the voice pipeline is running |
| `mode` | string | Current mode: `"conversation"`, `"meeting"`, or `"dictation"` |
| `duration` | number | Session duration in seconds |
| `segmentCount` | number | Number of transcript segments received |
| `currentlyListening` | boolean | Whether the mic is active |
| `currentlySpeaking` | boolean | Whether TTS is currently playing |

**When to use:**
- Check whether the voice channel is active before attempting voice operations
- Confirm which mode is currently active
- Monitor session duration for long meetings/dictations

---

### `voice_transcript` — Read, flush, or retrieve transcript history

**Parameters:** `action` (`"get"` | `"flush"` | `"history"`)

In **meeting** and **dictation** modes, transcribed text accumulates in a buffer rather than being emitted immediately. This tool lets you inspect or consume that buffer, or retrieve the full session transcript log.

| Action | Behavior |
|--------|----------|
| `get` | Returns the current buffer contents **without clearing** it |
| `flush` | Returns the current buffer contents **and clears** it |
| `history` | Returns the **full session transcript log** — all emitted blocks across all modes, with timestamps |

Returns `"(empty buffer)"` for `get`/`flush` if nothing has accumulated yet. Returns `"(no transcript history)"` for `history` if no blocks have been emitted.

**When to use:**
- Periodically check `get` during a long meeting to see what's been said so far
- Use `flush` when the user asks "what was said?" to deliver and clear the buffer
- Use `flush` when you need to process/summarize the accumulated text
- Use `history` to retrieve everything said in the session — useful for generating comprehensive meeting summaries, reviewing earlier context, or dumping the full transcript when the user asks
- In dictation mode, the buffer accumulates until the user says an end phrase (e.g., "end dictation" or "结束听写") — but you can also `flush` manually if needed

**When NOT to use:**
- In conversation mode, there is no meaningful buffer for `get`/`flush` — each turn is dispatched immediately (but `history` still works)

---

## Operating Modes in Detail

### Conversation Mode (default)

Standard turn-based voice dialogue. This is the default mode when the voice channel starts.

**How it works:**
1. User speaks → speech is transcribed in real time
2. When the user pauses (~700ms of silence), the turn is complete
3. The transcript is delivered to you as `[Voice input from microphone]: <text>`
4. You reply with text — and optionally call `voice_speak` to say a concise spoken response
5. If the user speaks during TTS playback, **barge-in** occurs: playback stops, the new utterance is captured

**Your responses are text-only by default.** Use `voice_speak` when you want to respond aloud. Keep spoken responses concise (1-3 sentences). Avoid markdown in spoken text. If detailed information is needed, speak a brief summary and let the full text appear in chat.

---

### Meeting Mode

Passive observation mode for listening to meetings, lectures, or multi-party discussions.

**How it works:**
1. User speaks (or multiple people speak in a meeting) → all speech is transcribed
2. Text accumulates in a buffer, segmented into **topic blocks** by:
   - **Topic shift detection**: Rust-side sentence embeddings (MiniLM-L12 v2) detect when the conversation topic changes. When cosine similarity to the running topic centroid drops below the threshold (~0.65), a new block begins.
   - **Silence timeout**: If nobody speaks for 30 seconds, the current block is emitted.
   - **Max block duration**: After 5 minutes, the block is force-emitted regardless.
   - **Auto-stop**: After 60 seconds of total silence, meeting mode auto-stops.
3. Each completed block is delivered to you as `[Meeting transcript block]: <text>`
4. **Your replies are NOT spoken aloud** — the meeting participants won't hear TTS.

**Keyword addressing:** If the user says a configured keyword (e.g., the agent's name), that utterance is dispatched **immediately** as `[Voice input from microphone]: <text>` with TTS enabled for your reply. This allows the user to ask you a question mid-meeting and get a spoken response, while regular meeting transcription remains silent.

**Auto-return:** When meeting mode auto-stops (60s silence) or is manually ended, the system automatically returns to the previous mode (usually conversation).

**What you should do in meeting mode:**
- Silently accumulate context from the transcript blocks
- When keyword-addressed, respond concisely (the meeting is ongoing)
- Use `voice_transcript({ action: "get" })` to preview the current buffer before it's emitted
- When the meeting ends, summarize key points, action items, or decisions

**Incremental summarization:** Each time a new transcript block arrives, review your previous summary against the new content. Update your running summary to incorporate new topics, decisions, and action items. Don't wait until the end — maintain a living summary throughout the meeting so you can respond accurately when keyword-addressed.

**Transcript history for review:** Use `voice_transcript({ action: "history" })` to retrieve the full session transcript at any time. This is useful when:
- The user asks for a comprehensive summary or transcript dump
- You need to cross-reference earlier discussion points with new blocks
- The meeting ends and you want to produce a final summary from all blocks
- The user asks you to save or export the meeting transcript (write it to a file, note, or other destination)

---

### Dictation Mode

Continuous capture mode for long-form text input.

**How it works:**
1. User speaks continuously → all final transcripts accumulate in a buffer
2. Silence does NOT trigger emission — the user can pause, think, and continue
3. The buffer is emitted when:
   - The user says an **end phrase**: `"end dictation"` or `"结束听写"` (configurable)
   - You call `voice_transcript({ action: "flush" })` manually
4. The accumulated text is delivered as `[Dictation result]: <text>`
5. **Your replies are NOT spoken aloud** — no TTS interruptions during dictation.

**Auto-return:** When the user says an end phrase (e.g., "end dictation" / "结束听写"), the system emits the buffer and automatically returns to the previous mode (usually conversation).

**What you should do in dictation mode:**
- Let the user dictate without interruption
- Use `voice_transcript({ action: "get" })` if the user asks "what do I have so far?"
- When the dictation ends (end phrase or flush), process the text as requested (format it, save it, send it, etc.)

**Polishing dictation output:** When you receive the dictation result, lightly polish it before presenting to the user:
- Fix obvious grammar and punctuation issues
- Remove filler words and false starts (e.g., "um", "uh", "I mean", "like")
- Add paragraph breaks where topic shifts naturally occur
- **Do NOT change the substance or meaning** — preserve the user's original wording, intent, and structure
- Present both the polished version and offer to show the raw transcript if the user wants it
- If the user specified a format (email, note, document), apply that formatting to the polished text

---

## Interaction Patterns

### Pattern: Conversational reply

```
User (voice): "What's the weather like today?"
→ voice_speak({ text: "It's sunny and 22 degrees in Shanghai." })
→ Text reply with more detail (forecast, humidity, etc.)
```

### Pattern: Meeting note-taking

```
User: "Start listening to my meeting"
→ voice_mode({ mode: "meeting" })
→ voice_speak({ text: "Meeting mode on. I'll listen silently." })

[... meeting transcript block 1 arrives ...]
→ (internally) Build initial summary from block 1

[... meeting transcript block 2 arrives ...]
→ (internally) Review previous summary, update with block 2's new topics/decisions

User (keyword): "Hey assistant, what's been discussed so far?"
→ voice_speak({ text: "So far you've covered the Q3 budget and the hiring plan." })

User: "The meeting is over"
→ voice_transcript({ action: "flush" })  // flush remaining buffer
→ voice_transcript({ action: "history" })  // get full transcript for final summary
→ voice_speak({ text: "Here's the meeting summary." })
→ Text reply with full summary, action items, decisions
(system auto-returns to previous mode)
```

### Pattern: Email dictation

```
User: "I want to dictate an email"
→ voice_mode({ mode: "dictation" })
→ voice_speak({ text: "Dictation mode. Speak freely — say 'end dictation' when done." })

[... user dictates ...]

User: "end dictation"
→ [Dictation result] arrives automatically
→ (system auto-returns to previous mode)
→ Polish the raw dictation: fix grammar, remove fillers, add paragraph breaks
→ voice_speak({ text: "Got your dictation. Here's the polished draft." })
→ Text reply with polished email (offer raw transcript if user wants it)
```

### Pattern: Code/technical question

```
User (voice): "How do I reverse a list in Python?"
→ voice_speak({ text: "You can use list.reverse() or slicing. Check the text for examples." })
→ Text reply with code examples (not spoken — code doesn't work well in TTS)
```

---

## Audio Pipeline Details

### Echo Cancellation
WebRTC AEC3 removes speaker output from the microphone signal. This prevents the system from transcribing its own TTS output as user speech.

### Barge-in Detection
A hybrid VAD gate requires ~200ms of sustained speech at elevated probability before confirming barge-in. This prevents false interruptions from brief noises or echo artifacts. When confirmed, TTS playback is immediately stopped and the user's new utterance is captured.

### Topic Detection (Meeting Mode)
The Rust audio subprocess runs a sentence embedding model (paraphrase-multilingual-MiniLM-L12-v2, ONNX) that:
- Embeds each final transcript into a 384-dim vector
- Maintains a running centroid via exponential moving average (alpha=0.3)
- Detects topic shifts when cosine similarity drops below threshold
- Supports both English and Chinese

### Cloud Providers

Currently supported: **Aliyun DashScope**

- **STT**: Real-time streaming recognition via WebSocket (model: `paraformer-realtime-v2`). Supports multi-language, punctuation prediction, disfluency removal.
- **TTS**: Streaming synthesis via WebSocket (model: `cosyvoice-v3-flash`). Supports multiple voices, speed control.
- **Fallback**: Local Whisper.cpp STT when no cloud provider is configured.

---

## Guidelines

### Voice response style
- Replies are text-only by default — use `voice_speak` to add a spoken response when appropriate
- When speaking, keep it concise — 1-3 sentences
- Avoid markdown formatting in spoken text (it won't be spoken naturally)
- Don't reference visual elements the user can't see in spoken responses
- If a response requires detailed information (code, lists, links), speak a brief summary and let the full text appear in chat

### Proactive mode switching

When the user describes an upcoming situation, **proactively suggest or switch to the appropriate mode** — don't wait for explicit commands like "switch to meeting mode."

**Triggers for meeting mode** — switch when the user mentions:
- Meetings, calls, interviews, discussions, standups, syncs, 1:1s, reviews
- Lectures, presentations, talks, webinars, classes
- "I'm about to…", "starting a…", "joining a…", "hopping on a…"
- Someone else is about to speak or multiple people will be talking
- Example: "I have an interview in 5 minutes" → suggest meeting mode

**Triggers for dictation mode** — switch when the user mentions:
- Writing or dictating: emails, letters, notes, documents, messages, reports
- "Let me dictate…", "I want to write…", "take this down…"
- Example: "I need to draft a reply to this email" → suggest dictation mode

**How to suggest:** Briefly explain what the mode does and ask, or just switch if the intent is unambiguous:
```
User: "I'm about to do an interview"
→ voice_mode({ mode: "meeting" })
→ voice_speak({ text: "Switched to meeting mode — I'll listen silently and take notes. Say my name if you need me." })
```

If the intent is ambiguous (e.g., "I have a call" could mean the user wants help preparing vs. being listened to), ask first:
```
→ voice_speak({ text: "Want me to listen in on the call? I can switch to meeting mode and take notes silently." })
```

### Mode-specific behavior
- In meeting mode, stay silent unless keyword-addressed — don't interrupt the meeting
- In dictation mode, don't speak at all — let the user dictate uninterrupted
- When switching modes, briefly confirm the switch so the user knows

### Tool usage
- Use `voice_speak` whenever you want the user to hear a spoken response — this is the only way to produce voice output
- Don't speak your entire text reply — speak a concise version and let the full text appear in chat
- Use `voice_listen` to mute the mic during long processing
- Use `voice_transcript` to preview buffers in meeting/dictation before they auto-emit
- Always check `voice_status` before attempting operations if unsure whether the channel is active
