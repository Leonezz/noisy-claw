pub mod aec;
pub mod capture;
pub mod output;
pub mod stt;
pub mod tts;
pub mod vad;

/// Audio data flowing between pipeline nodes.
#[derive(Clone)]
pub struct AudioFrame {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// VAD state transition emitted by the VAD node.
pub struct VadEvent {
    pub speaking: bool,
}

/// Speech recognition result emitted by the STT node.
pub struct TranscriptEvent {
    pub text: String,
    pub is_final: bool,
    pub start: f64,
    pub end: f64,
    pub confidence: Option<f64>,
}

/// Messages from the output node back to the orchestrator.
pub enum OutputNodeEvent {
    SpeakDone,
}

/// Messages sent to the output node (from TTS node and orchestrator).
pub enum OutputMessage {
    /// Begin a new playback session at the given sample rate.
    StartSession { sample_rate: u32 },
    /// PCM audio chunk to write to the ring buffer.
    AudioChunk(Vec<f32>),
    /// All audio chunks have been sent; wait for buffer drain.
    FinishSession,
    /// Stop immediately (interruption / barge-in).
    StopSession,
}
