import { describe, it, expect, vi, beforeEach } from "vitest";
import { PipelineCoordinator, type PipelineComponents } from "./coordinator.js";
import type {
  AudioSource,
  STTProvider,
  SegmentationEngine,
  AudioOutput,
  AudioChunk,
  TranscriptSegment,
  SegmentMetadata,
} from "./interfaces.js";

function createMockComponents(): PipelineComponents & {
  _audioVadCbs: Array<(speaking: boolean) => void>;
  _audioChunkCbs: Array<(chunk: AudioChunk) => void>;
  _sttTranscriptCbs: Array<(segment: TranscriptSegment) => void>;
  _segMessageCbs: Array<(msg: string, meta: SegmentMetadata) => void>;
  _outputDoneCbs: Array<(requestId: string, reason: string) => void>;
} {
  const audioVadCbs: Array<(speaking: boolean) => void> = [];
  const audioChunkCbs: Array<(chunk: AudioChunk) => void> = [];
  const sttTranscriptCbs: Array<(segment: TranscriptSegment) => void> = [];
  const segMessageCbs: Array<(msg: string, meta: SegmentMetadata) => void> = [];
  const outputDoneCbs: Array<(requestId: string, reason: string) => void> = [];

  const audioSource: AudioSource = {
    start: vi.fn(),
    stop: vi.fn(),
    onAudio: (cb) => audioChunkCbs.push(cb),
    onVAD: (cb) => audioVadCbs.push(cb),
  };

  const sttProvider: STTProvider = {
    start: vi.fn(),
    stop: vi.fn(),
    feed: vi.fn(),
    onTranscript: (cb) => sttTranscriptCbs.push(cb),
  };

  const segmentation: SegmentationEngine = {
    onTranscript: vi.fn(),
    onVAD: vi.fn(),
    onMessage: (cb) => segMessageCbs.push(cb),
    flush: vi.fn(() => null),
  };

  const audioOutput: AudioOutput = {
    speak: vi.fn(),
    speakStart: vi.fn(),
    speakChunk: vi.fn(),
    speakEnd: vi.fn(),
    stop: vi.fn(),
    flush: vi.fn(),
    isPlaying: vi.fn(() => false),
    onDone: (cb) => outputDoneCbs.push(cb),
  };

  return {
    audioSource,
    sttProvider,
    segmentation,
    audioOutput,
    _audioVadCbs: audioVadCbs,
    _audioChunkCbs: audioChunkCbs,
    _sttTranscriptCbs: sttTranscriptCbs,
    _segMessageCbs: segMessageCbs,
    _outputDoneCbs: outputDoneCbs,
  };
}

describe("PipelineCoordinator", () => {
  let mocks: ReturnType<typeof createMockComponents>;
  let coordinator: PipelineCoordinator;

  beforeEach(() => {
    mocks = createMockComponents();
    coordinator = new PipelineCoordinator(mocks);
  });

  it("starts audio source and STT on start()", () => {
    coordinator.start({
      audio: { device: "default", sampleRate: 48000 },
      stt: { model: "base", language: "en" },
    });

    expect(mocks.audioSource.start).toHaveBeenCalledWith({
      device: "default",
      sampleRate: 48000,
    });
    expect(mocks.sttProvider.start).toHaveBeenCalledWith({
      model: "base",
      language: "en",
    });
  });

  it("does not start twice", () => {
    const config = {
      audio: { device: "default", sampleRate: 48000 },
      stt: { model: "base", language: "en" },
    };
    coordinator.start(config);
    coordinator.start(config);

    expect(mocks.audioSource.start).toHaveBeenCalledTimes(1);
  });

  it("stops and flushes on stop()", () => {
    coordinator.start({
      audio: { device: "default", sampleRate: 48000 },
      stt: { model: "base", language: "en" },
    });
    coordinator.stop();

    expect(mocks.audioSource.stop).toHaveBeenCalled();
    expect(mocks.sttProvider.stop).toHaveBeenCalled();
    expect(mocks.segmentation.flush).toHaveBeenCalled();
  });

  it("forwards VAD events to segmentation", () => {
    for (const cb of mocks._audioVadCbs) {
      cb(true);
    }
    expect(mocks.segmentation.onVAD).toHaveBeenCalledWith(true);
  });

  it("forwards audio chunks to STT", () => {
    const chunk: AudioChunk = {
      data: Buffer.from([]),
      timestamp: 0,
    };
    for (const cb of mocks._audioChunkCbs) {
      cb(chunk);
    }
    expect(mocks.sttProvider.feed).toHaveBeenCalledWith(chunk);
  });

  it("forwards transcript segments to segmentation", () => {
    const segment: TranscriptSegment = {
      text: "hello",
      isFinal: true,
      start: 0,
      end: 1,
    };
    for (const cb of mocks._sttTranscriptCbs) {
      cb(segment);
    }
    expect(mocks.segmentation.onTranscript).toHaveBeenCalledWith(segment);
  });

  it("forwards segmentation messages to coordinator callbacks", () => {
    const messageCb = vi.fn();
    coordinator.onMessage(messageCb);

    const metadata: SegmentMetadata = {
      startTime: 0,
      endTime: 1,
      segmentCount: 1,
    };
    for (const cb of mocks._segMessageCbs) {
      cb("hello world", metadata);
    }

    expect(messageCb).toHaveBeenCalledWith("hello world", metadata);
  });

  it("speak() delegates to audioOutput.speak() with requestId", () => {
    coordinator.speak("test message");

    expect(mocks.audioOutput.speak).toHaveBeenCalledWith(
      "test message",
      expect.stringMatching(/^req-ts-/),
    );
  });

  it("speakStart/speakChunk/speakEnd flow with sentence chunking", () => {
    coordinator.speakStart();
    expect(mocks.audioOutput.speakStart).toHaveBeenCalledWith(
      expect.stringMatching(/^req-ts-/),
    );

    // Send partial text (no sentence boundary yet)
    coordinator.speakChunk("Hello wor");
    expect(mocks.audioOutput.speakChunk).not.toHaveBeenCalled();

    // Complete the sentence with boundary
    coordinator.speakChunk("ld. How ");
    expect(mocks.audioOutput.speakChunk).toHaveBeenCalledTimes(1);
    expect(mocks.audioOutput.speakChunk).toHaveBeenCalledWith(
      "Hello world.",
      expect.stringMatching(/^req-ts-/),
    );

    // speakEnd flushes remaining buffer (includes leading space from split)
    coordinator.speakEnd();
    expect(mocks.audioOutput.speakChunk).toHaveBeenCalledTimes(2);
    expect(mocks.audioOutput.speakChunk).toHaveBeenLastCalledWith(
      " How ",
      expect.stringMatching(/^req-ts-/),
    );
    expect(mocks.audioOutput.speakEnd).toHaveBeenCalled();
  });

  it("onDone callback resets activeRequestId", () => {
    coordinator.speak("response");

    // Simulate done
    for (const cb of mocks._outputDoneCbs) {
      cb("req-ts-000001", "completed");
    }

    // Audio chunks should still flow to STT (no echo suppression in TS anymore)
    const chunk: AudioChunk = { data: Buffer.from([]), timestamp: 0 };
    for (const cb of mocks._audioChunkCbs) {
      cb(chunk);
    }
    expect(mocks.sttProvider.feed).toHaveBeenCalledWith(chunk);
  });

  it("isActive reflects pipeline state", () => {
    expect(coordinator.isActive).toBe(false);
    coordinator.start({
      audio: { device: "default", sampleRate: 48000 },
      stt: { model: "base", language: "en" },
    });
    expect(coordinator.isActive).toBe(true);
    coordinator.stop();
    expect(coordinator.isActive).toBe(false);
  });

  it("isSpeaking delegates to audio output", () => {
    expect(coordinator.isSpeaking).toBe(false);
  });
});
