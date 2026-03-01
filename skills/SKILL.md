---
name: skills
description: Control the voice channel — speak, listen, switch modes, check status
---

# Voice Channel

You have access to a voice channel that lets users speak to you and hear your responses. The system uses real-time speech-to-text, text-to-speech, and echo cancellation for natural conversation.

## Tools

- `voice_speak`: Synthesize and play text aloud through the voice channel. Input: `text` (string). Only works when the voice pipeline is active.
- `voice_listen`: Start or stop microphone listening. Input: `listening` (boolean). When false, pauses capture; when true, resumes.
- `voice_mode`: Switch the voice channel mode (conversation/listen/dictation). Currently only "conversation" is supported.
- `voice_status`: Query current voice session state. Returns: `active`, `mode`, `duration`, `segmentCount`, `currentlyListening`, `currentlySpeaking`.

## Behavior

When the voice channel is active:

- User speech is automatically transcribed and delivered to you as text messages
- Your text responses are streamed sentence-by-sentence to TTS for low-latency playback (splits on `.!?` and CJK sentence-ending punctuation)
- The text version of your response is also visible in the chat
- If the user speaks while you're responding, playback stops (barge-in) and a new turn begins
- Echo cancellation (WebRTC AEC3) removes speaker output from the microphone signal, preventing the system from hearing its own voice
- A hybrid VAD gate requires ~200ms of sustained speech at elevated threshold before confirming barge-in, preventing false interruptions from background noise

## Cloud Providers

Currently supported: **Aliyun DashScope**

- **STT**: Real-time streaming recognition via WebSocket (default model: `paraformer-realtime-v2`). Supports multi-language, punctuation prediction, disfluency removal.
- **TTS**: Streaming synthesis via WebSocket (default model: `cosyvoice-v3-flash`). Supports multiple voices, speed control, PCM/MP3/WAV output.
- **Fallback**: Local Whisper.cpp STT when no cloud provider is configured.

## Guidelines

- Keep voice responses concise — aim for 1-3 sentences when possible
- Avoid markdown formatting in voice responses (it won't be spoken naturally)
- Don't reference visual elements the user can't see
- If a response requires detailed information (code, lists, links), suggest the user check the text version
- Use `voice_speak` to say something independently of your text reply (e.g., acknowledgements, greetings)
- Use `voice_listen` to temporarily mute the mic when not expecting user input
