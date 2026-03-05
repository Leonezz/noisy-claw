export type VoiceMode = "conversation" | "meeting" | "dictation";

export type TranscriptEntry = {
  text: string;
  mode: VoiceMode;
  timestamp: number; // Unix timestamp ms
  startTime: number; // audio start (seconds from capture start)
  endTime: number; // audio end (seconds from capture start)
};

export type VoiceSessionState = {
  active: boolean;
  mode: VoiceMode;
  startTime: number | null; // Unix timestamp ms
  segmentCount: number;
  currentlyListening: boolean;
  currentlySpeaking: boolean;
};

const INITIAL_STATE: VoiceSessionState = {
  active: false,
  mode: "conversation",
  startTime: null,
  segmentCount: 0,
  currentlyListening: false,
  currentlySpeaking: false,
};

export class VoiceSession {
  private state: VoiceSessionState = { ...INITIAL_STATE };
  private transcriptLog: TranscriptEntry[] = [];

  start(): VoiceSessionState {
    return {
      ...this.state,
      active: true,
      startTime: Date.now(),
      segmentCount: 0,
      currentlyListening: true,
    };
  }

  stop(): VoiceSessionState {
    return {
      ...this.state,
      active: false,
      startTime: null,
      currentlyListening: false,
      currentlySpeaking: false,
    };
  }

  setMode(mode: VoiceMode): VoiceSessionState {
    return { ...this.state, mode };
  }

  incrementSegments(): VoiceSessionState {
    return { ...this.state, segmentCount: this.state.segmentCount + 1 };
  }

  setSpeaking(speaking: boolean): VoiceSessionState {
    return { ...this.state, currentlySpeaking: speaking };
  }

  setListening(listening: boolean): VoiceSessionState {
    return { ...this.state, currentlyListening: listening };
  }

  getState(): Readonly<VoiceSessionState> {
    return this.state;
  }

  getDuration(): number {
    if (!this.state.startTime) return 0;
    return (Date.now() - this.state.startTime) / 1000;
  }

  update(next: VoiceSessionState): void {
    this.state = next;
  }

  /** Append a transcript block to the session history. */
  logTranscript(text: string, startTime: number, endTime: number): void {
    this.transcriptLog.push({
      text,
      mode: this.state.mode,
      timestamp: Date.now(),
      startTime,
      endTime,
    });
  }

  /** Get the full transcript history for this session. */
  getTranscriptHistory(): readonly TranscriptEntry[] {
    return this.transcriptLog;
  }

  /** Clear transcript history (e.g., on session stop). */
  clearTranscriptHistory(): void {
    this.transcriptLog = [];
  }
}
