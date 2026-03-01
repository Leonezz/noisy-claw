import type { PluginRuntime } from "openclaw/plugin-sdk";
import type { PipelineCoordinator } from "../pipeline/coordinator.js";
import type { SegmentMetadata } from "../pipeline/interfaces.js";

export type VoiceDispatchDeps = {
  runtime: PluginRuntime;
  cfg: Record<string, unknown>;
  accountId: string;
  getPipeline: () => PipelineCoordinator | null;
};

// Sentence boundary: split at .!? and CJK sentence-end marks, plus newlines
const SENTENCE_END = /[.!?。！？\n]/;

/**
 * Dispatch a voice transcript to the OpenClaw agent as an inbound message.
 * Streams the AI reply sentence-by-sentence to TTS for low-latency playback.
 */
export async function dispatchVoiceTranscript(
  deps: VoiceDispatchDeps,
  transcript: string,
  _metadata: SegmentMetadata,
): Promise<void> {
  const { runtime, cfg, accountId } = deps;

  const sessionId = `voice-${accountId}`;
  const from = `voice:${sessionId}`;

  const route = runtime.channel.routing.resolveAgentRoute({
    cfg: cfg as Parameters<typeof runtime.channel.routing.resolveAgentRoute>[0]["cfg"],
    channel: "voice",
    accountId,
    peer: { kind: "direct", id: sessionId },
  });

  const msgCtx = {
    Body: transcript,
    BodyForAgent: `[Voice input from microphone]: ${transcript}`,
    RawBody: transcript,
    CommandBody: transcript,
    Transcript: transcript,
    From: from,
    To: from,
    SessionKey: route.sessionKey,
    AccountId: accountId,
    Provider: "voice",
    Surface: "voice",
    OriginatingChannel: "voice" as const,
    OriginatingTo: from,
    ChatType: "direct",
    MessageSid: `voice-${Date.now()}`,
    Timestamp: Date.now(),
    CommandAuthorized: true,
  };

  const finalizedCtx = runtime.channel.reply.finalizeInboundContext(msgCtx);

  // Streaming state for sentence-boundary TTS
  let streamStarted = false;
  let sentCursor = 0;

  function flushSentences(fullText: string, force: boolean): void {
    const pipeline = deps.getPipeline();
    if (!pipeline?.isActive) return;

    while (sentCursor < fullText.length) {
      const remaining = fullText.slice(sentCursor);
      const match = SENTENCE_END.exec(remaining);
      if (match) {
        const end = sentCursor + match.index + match[0].length;
        const sentence = fullText.slice(sentCursor, end).trim();
        if (sentence) {
          if (!streamStarted) {
            pipeline.speakStart();
            streamStarted = true;
          }
          pipeline.speakChunk(sentence);
        }
        sentCursor = end;
      } else if (force) {
        const tail = remaining.trim();
        if (tail) {
          if (!streamStarted) {
            pipeline.speakStart();
            streamStarted = true;
          }
          pipeline.speakChunk(tail);
        }
        sentCursor = fullText.length;
      } else {
        break;
      }
    }
  }

  await runtime.channel.reply.dispatchReplyWithBufferedBlockDispatcher({
    ctx: finalizedCtx,
    cfg: cfg as Parameters<
      typeof runtime.channel.reply.dispatchReplyWithBufferedBlockDispatcher
    >[0]["cfg"],
    dispatcherOptions: {
      deliver: async (payload, _info) => {
        const text = payload.text?.trim();
        if (text) {
          console.log(
            `[noisy-claw] agent reply: ${text.slice(0, 100)}${text.length > 100 ? "…" : ""}`,
          );
          // Final delivery per block — flush remaining text
          flushSentences(text, true);
        }
      },
      onError: (err) => {
        console.error("[noisy-claw] dispatch error:", err);
      },
    },
    replyOptions: {
      onPartialReply: (payload: { text?: string }) => {
        if (payload.text) {
          flushSentences(payload.text, false);
        }
      },
    },
  });

  // After all blocks delivered, signal end of TTS stream
  if (streamStarted) {
    const pipeline = deps.getPipeline();
    if (pipeline?.isActive) {
      await pipeline.speakEnd();
    }
  }
}
