// --- Config types for cloud providers ---

export type SttConfig = {
  provider: string;
  api_key?: string;
  endpoint?: string;
  model?: string;
  languages?: string[];
  extra?: Record<string, string>;
};

export type TtsConfig = {
  provider: string;
  api_key?: string;
  endpoint?: string;
  model?: string;
  voice?: string;
  format?: string;
  sample_rate?: number;
  speed?: number;
  extra?: Record<string, string>;
};

// Commands sent from Node.js to Rust (via stdin)
export type StartCaptureCommand = {
  cmd: "start_capture";
  device?: string;
  sample_rate?: number;
  stt?: SttConfig;
};

export type StopCaptureCommand = {
  cmd: "stop_capture";
};

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
};

export type SpeakEndCommand = {
  cmd: "speak_end";
};

export type FlushSpeakCommand = {
  cmd: "flush_speak";
  request_id: string;
};

export type StopSpeakingCommand = {
  cmd: "stop_speaking";
};

export type PlayAudioCommand = {
  cmd: "play_audio";
  path: string;
};

export type StopPlaybackCommand = {
  cmd: "stop_playback";
};

export type GetStatusCommand = {
  cmd: "get_status";
};

export type SetModeCommand = {
  cmd: "set_mode";
  mode: string;
};

export type ShutdownCommand = {
  cmd: "shutdown";
};

export type Command =
  | StartCaptureCommand
  | StopCaptureCommand
  | SpeakCommand
  | SpeakStartCommand
  | SpeakChunkCommand
  | SpeakEndCommand
  | StopSpeakingCommand
  | FlushSpeakCommand
  | PlayAudioCommand
  | StopPlaybackCommand
  | SetModeCommand
  | GetStatusCommand
  | ShutdownCommand;

// Events received from Rust (via stdout)
export type ReadyEvent = {
  event: "ready";
};

export type VadEvent = {
  event: "vad";
  speaking: boolean;
};

export type TranscriptEvent = {
  event: "transcript";
  text: string;
  is_final: boolean;
  start: number;
  end: number;
  confidence?: number;
};

export type SpeakStartedEvent = {
  event: "speak_started";
  request_id?: string;
};

export type SpeakDoneEvent = {
  event: "speak_done";
  request_id?: string;
  reason: string;
};

export type PlaybackDoneEvent = {
  event: "playback_done";
};

export type StatusEvent = {
  event: "status";
  capturing: boolean;
  playing: boolean;
  speaking: boolean;
};

export type TopicShiftEvent = {
  event: "topic_shift";
  similarity: number;
};

export type ErrorEvent = {
  event: "error";
  message: string;
};

export type AudioEvent =
  | ReadyEvent
  | VadEvent
  | TranscriptEvent
  | SpeakStartedEvent
  | SpeakDoneEvent
  | TopicShiftEvent
  | PlaybackDoneEvent
  | StatusEvent
  | ErrorEvent;

export function parseEvent(line: string): AudioEvent | null {
  try {
    const parsed: unknown = JSON.parse(line);
    if (
      typeof parsed !== "object" ||
      parsed === null ||
      typeof (parsed as Record<string, unknown>).event !== "string"
    ) {
      return null;
    }
    return parsed as AudioEvent;
  } catch {
    return null;
  }
}

export function serializeCommand(cmd: Command): string {
  return JSON.stringify(cmd);
}
