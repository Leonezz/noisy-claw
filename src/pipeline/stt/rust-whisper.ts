import type { AudioEvent } from "../../ipc/protocol.js";
import type { STTProvider, STTConfig, AudioChunk, TranscriptSegment } from "../interfaces.js";

export class RustWhisperSTT implements STTProvider {
  private transcriptCallbacks: Array<(segment: TranscriptSegment) => void> = [];

  start(_config: STTConfig): void {
    // STT is started implicitly when capture starts in the Rust process.
    // The config (model, language) is set at subprocess spawn time.
  }

  stop(): void {
    // STT stops when capture stops.
  }

  feed(_chunk: AudioChunk): void {
    // In the Rust subprocess architecture, audio flows directly from
    // capture -> VAD -> STT internally. We don't feed chunks from TS.
    // This method exists for future cloud STT providers that receive chunks from TS.
  }

  onTranscript(cb: (segment: TranscriptSegment) => void): void {
    this.transcriptCallbacks.push(cb);
  }

  /** Called by the coordinator when IPC events arrive. */
  handleEvent(event: AudioEvent): void {
    if (event.event === "transcript") {
      const segment: TranscriptSegment = {
        text: event.text,
        isFinal: event.is_final,
        start: event.start,
        end: event.end,
        confidence: event.confidence,
      };
      for (const cb of this.transcriptCallbacks) {
        cb(segment);
      }
    }
  }
}
