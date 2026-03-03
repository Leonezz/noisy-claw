pub mod aec;
pub mod capture;
pub mod output;
pub mod stt;
pub mod tts;
pub mod vad;

/// Identifies a node in the pipeline for flush acknowledgment.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeId {
    Capture,
    Aec,
    Vad,
    Stt,
    Tts,
    Output,
}

/// Opaque request identifier for tracking audio through the pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub String);

/// Signal to flush buffered data for a specific request or all requests.
pub enum FlushSignal {
    Flush { request_id: String },
    FlushAll,
}

/// Acknowledgment that a node has completed flushing.
pub struct FlushAck {
    pub node: NodeId,
    pub request_id: Option<String>,
}

/// Common trait for all pipeline nodes.
///
/// Domain-specific commands remain in per-node `Control` enums.
/// This trait enables future graph-based orchestration and provides
/// a uniform flush/shutdown protocol.
#[async_trait::async_trait]
pub trait PipelineNode: Send + 'static {
    fn node_id(&self) -> NodeId;
    async fn flush(&mut self, signal: FlushSignal) -> FlushAck;
    async fn shutdown(&mut self);
}

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

/// Messages from the output node back to the orchestrator.
pub enum OutputNodeEvent {
    SpeakDone,
}

/// Messages sent to the output node (from TTS node and orchestrator).
pub enum OutputMessage {
    /// Begin a new playback session at the given sample rate.
    StartSession { request_id: RequestId, sample_rate: u32 },
    /// PCM audio chunk to write to the ring buffer.
    AudioChunk { request_id: RequestId, samples: Vec<f32>, sample_rate: u32 },
    /// All audio chunks have been sent; wait for buffer drain.
    FinishSession { request_id: RequestId },
    /// Stop a specific request immediately (interruption / barge-in).
    StopSession { request_id: RequestId },
    /// Stop all active sessions.
    StopAll,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_eq_and_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(NodeId::Capture);
        set.insert(NodeId::Tts);
        assert!(set.contains(&NodeId::Capture));
        assert!(!set.contains(&NodeId::Vad));
    }

    #[test]
    fn request_id_clone_and_eq() {
        let id1 = RequestId("req-001".to_string());
        let id2 = id1.clone();
        assert_eq!(id1, id2);
    }

    #[test]
    fn flush_ack_carries_node_id() {
        let ack = FlushAck {
            node: NodeId::Output,
            request_id: Some("req-001".to_string()),
        };
        assert_eq!(ack.node, NodeId::Output);
        assert_eq!(ack.request_id, Some("req-001".to_string()));
    }

    #[test]
    fn output_message_audio_chunk_carries_metadata() {
        let msg = OutputMessage::AudioChunk {
            request_id: RequestId("req-001".to_string()),
            samples: vec![0.1, 0.2],
            sample_rate: 16000,
        };
        match msg {
            OutputMessage::AudioChunk { request_id, samples, sample_rate } => {
                assert_eq!(request_id, RequestId("req-001".to_string()));
                assert_eq!(samples.len(), 2);
                assert_eq!(sample_rate, 16000);
            }
            _ => panic!("expected AudioChunk"),
        }
    }

    #[test]
    fn output_message_start_session_carries_request_id() {
        let msg = OutputMessage::StartSession {
            request_id: RequestId("req-002".to_string()),
            sample_rate: 24000,
        };
        match msg {
            OutputMessage::StartSession { request_id, sample_rate } => {
                assert_eq!(request_id, RequestId("req-002".to_string()));
                assert_eq!(sample_rate, 24000);
            }
            _ => panic!("expected StartSession"),
        }
    }
}
