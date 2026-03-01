// Commands sent from Node.js to Rust (via stdin)
export type StartCaptureCommand = {
  cmd: "start_capture";
  device?: string;
  sample_rate?: number;
};

export type StopCaptureCommand = {
  cmd: "stop_capture";
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

export type ShutdownCommand = {
  cmd: "shutdown";
};

export type Command =
  | StartCaptureCommand
  | StopCaptureCommand
  | PlayAudioCommand
  | StopPlaybackCommand
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

export type PlaybackDoneEvent = {
  event: "playback_done";
};

export type StatusEvent = {
  event: "status";
  capturing: boolean;
  playing: boolean;
};

export type ErrorEvent = {
  event: "error";
  message: string;
};

export type AudioEvent =
  | ReadyEvent
  | VadEvent
  | TranscriptEvent
  | PlaybackDoneEvent
  | StatusEvent
  | ErrorEvent;

export function parseEvent(line: string): AudioEvent | null {
  try {
    return JSON.parse(line) as AudioEvent;
  } catch {
    return null;
  }
}

export function serializeCommand(cmd: Command): string {
  return JSON.stringify(cmd);
}
