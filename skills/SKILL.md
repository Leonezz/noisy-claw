---
name: skills
description: Control the voice channel — start/stop listening, switch modes, check status
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
