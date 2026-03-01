export type VoiceMode = "conversation" | "listen" | "dictation";

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
}
