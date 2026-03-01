//! Concrete types for the DashScope WebSocket protocol.
//!
//! Requests use `"action"` in the header; responses use `"event"`.
//! These types make that distinction explicit at compile time.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Request types (Serialize)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct DashScopeRequest<P: Serialize> {
    pub header: RequestHeader,
    pub payload: P,
}

#[derive(Serialize)]
pub struct RequestHeader {
    pub action: RequestAction,
    pub task_id: String,
    pub streaming: String,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
#[allow(clippy::enum_variant_names)] // names mirror DashScope protocol actions
pub enum RequestAction {
    RunTask,
    ContinueTask,
    FinishTask,
}

// -- Payloads ---------------------------------------------------------------

#[derive(Serialize)]
pub struct RunTaskPayload<P: Serialize> {
    pub task_group: String,
    pub task: String,
    pub function: String,
    pub model: String,
    pub parameters: P,
    pub input: EmptyInput,
}

#[derive(Serialize)]
pub struct EmptyInput {}

#[derive(Serialize)]
pub struct AsrParameters {
    pub format: String,
    pub sample_rate: u32,
    pub language_hints: Vec<String>,
    pub disfluency_removal_enabled: bool,
    pub semantic_punctuation_enabled: bool,
    pub punctuation_prediction_enabled: bool,
    pub max_sentence_silence: u32,
    pub multi_threshold_mode_enabled: bool,
    pub heartbeat: bool,
}

#[derive(Serialize)]
pub struct TtsParameters {
    pub voice: String,
    pub format: String,
    pub sample_rate: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
}

#[derive(Serialize)]
pub struct ContinueTaskPayload {
    pub input: TextInput,
}

#[derive(Serialize)]
pub struct TextInput {
    pub text: String,
}

#[derive(Serialize)]
pub struct FinishTaskPayload {
    pub input: EmptyInput,
}

// ---------------------------------------------------------------------------
// Builder functions — return serialized JSON strings
// ---------------------------------------------------------------------------

pub fn run_task_asr(task_id: &str, model: &str, params: AsrParameters) -> String {
    let req = DashScopeRequest {
        header: RequestHeader {
            action: RequestAction::RunTask,
            task_id: task_id.to_string(),
            streaming: "duplex".to_string(),
        },
        payload: RunTaskPayload {
            task_group: "audio".to_string(),
            task: "asr".to_string(),
            function: "recognition".to_string(),
            model: model.to_string(),
            parameters: params,
            input: EmptyInput {},
        },
    };
    serde_json::to_string(&req).expect("request serialization cannot fail")
}

pub fn run_task_tts(task_id: &str, model: &str, params: TtsParameters) -> String {
    let req = DashScopeRequest {
        header: RequestHeader {
            action: RequestAction::RunTask,
            task_id: task_id.to_string(),
            streaming: "duplex".to_string(),
        },
        payload: RunTaskPayload {
            task_group: "audio".to_string(),
            task: "tts".to_string(),
            function: "SpeechSynthesizer".to_string(),
            model: model.to_string(),
            parameters: params,
            input: EmptyInput {},
        },
    };
    serde_json::to_string(&req).expect("request serialization cannot fail")
}

pub fn continue_task(task_id: &str, text: &str) -> String {
    let req = DashScopeRequest {
        header: RequestHeader {
            action: RequestAction::ContinueTask,
            task_id: task_id.to_string(),
            streaming: "duplex".to_string(),
        },
        payload: ContinueTaskPayload {
            input: TextInput {
                text: text.to_string(),
            },
        },
    };
    serde_json::to_string(&req).expect("request serialization cannot fail")
}

pub fn finish_task(task_id: &str) -> String {
    let req = DashScopeRequest {
        header: RequestHeader {
            action: RequestAction::FinishTask,
            task_id: task_id.to_string(),
            streaming: "duplex".to_string(),
        },
        payload: FinishTaskPayload {
            input: EmptyInput {},
        },
    };
    serde_json::to_string(&req).expect("request serialization cannot fail")
}

// ---------------------------------------------------------------------------
// Response types (Deserialize)
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
pub struct DashScopeResponse {
    pub header: ResponseHeader,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Deserialize, Debug)]
pub struct ResponseHeader {
    pub task_id: String,
    pub event: String,
    pub code: Option<String>,
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Parsed event enum
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum DashScopeEvent {
    TaskStarted {
        task_id: String,
    },
    ResultGenerated {
        task_id: String,
        payload: serde_json::Value,
    },
    TaskFinished {
        task_id: String,
    },
    TaskFailed {
        task_id: String,
        code: String,
        message: String,
    },
    Unknown {
        task_id: String,
        event: String,
    },
}

// ---------------------------------------------------------------------------
// ASR sentence extractor
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
pub struct AsrSentence {
    pub text: String,
    pub begin_time: f64,
    pub end_time: f64,
    #[serde(default)]
    pub sentence_end: bool,
}

impl DashScopeEvent {
    /// Extract an ASR sentence from a `ResultGenerated` payload, if present.
    pub fn as_asr_sentence(&self) -> Option<AsrSentence> {
        if let DashScopeEvent::ResultGenerated { payload, .. } = self {
            let sentence = payload.get("output")?.get("sentence")?;
            serde_json::from_value(sentence.clone()).ok()
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Parse helper
// ---------------------------------------------------------------------------

pub fn parse_event(text: &str) -> anyhow::Result<DashScopeEvent> {
    let resp: DashScopeResponse =
        serde_json::from_str(text).map_err(|e| anyhow::anyhow!("invalid JSON from DashScope: {e}"))?;

    let task_id = resp.header.task_id;
    let event = match resp.header.event.as_str() {
        "task-started" => DashScopeEvent::TaskStarted { task_id },
        "result-generated" => DashScopeEvent::ResultGenerated {
            task_id,
            payload: resp.payload,
        },
        "task-finished" => DashScopeEvent::TaskFinished { task_id },
        "task-failed" => DashScopeEvent::TaskFailed {
            task_id,
            code: resp.header.code.unwrap_or_default(),
            message: resp.header.message.unwrap_or_default(),
        },
        other => DashScopeEvent::Unknown {
            task_id,
            event: other.to_string(),
        },
    };
    Ok(event)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_task_asr_serialization() {
        let json_str = run_task_asr(
            "tid-123",
            "paraformer-realtime-v2",
            AsrParameters {
                format: "pcm".to_string(),
                sample_rate: 16000,
                language_hints: vec!["zh".to_string(), "en".to_string()],
                disfluency_removal_enabled: true,
                semantic_punctuation_enabled: true,
                punctuation_prediction_enabled: true,
                max_sentence_silence: 800,
                multi_threshold_mode_enabled: true,
                heartbeat: true,
            },
        );
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["header"]["action"], "run-task");
        assert_eq!(v["header"]["task_id"], "tid-123");
        assert_eq!(v["header"]["streaming"], "duplex");
        assert_eq!(v["payload"]["task"], "asr");
        assert_eq!(v["payload"]["function"], "recognition");
        assert_eq!(v["payload"]["model"], "paraformer-realtime-v2");
        assert_eq!(v["payload"]["parameters"]["sample_rate"], 16000);
        assert_eq!(v["payload"]["parameters"]["language_hints"][0], "zh");
        assert_eq!(v["payload"]["parameters"]["disfluency_removal_enabled"], true);
        assert_eq!(v["payload"]["parameters"]["semantic_punctuation_enabled"], true);
        assert_eq!(v["payload"]["parameters"]["punctuation_prediction_enabled"], true);
        assert_eq!(v["payload"]["parameters"]["max_sentence_silence"], 800);
        assert_eq!(v["payload"]["parameters"]["multi_threshold_mode_enabled"], true);
        assert_eq!(v["payload"]["parameters"]["heartbeat"], true);
    }

    #[test]
    fn run_task_tts_serialization() {
        let json_str = run_task_tts(
            "tid-456",
            "cosyvoice-v3-flash",
            TtsParameters {
                voice: "longxiaochun".to_string(),
                format: "mp3".to_string(),
                sample_rate: 22050,
                rate: None,
            },
        );
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["header"]["action"], "run-task");
        assert_eq!(v["payload"]["task"], "tts");
        assert_eq!(v["payload"]["function"], "SpeechSynthesizer");
        assert_eq!(v["payload"]["parameters"]["voice"], "longxiaochun");
        // rate omitted when None
        assert!(v["payload"]["parameters"].get("rate").is_none());
    }

    #[test]
    fn run_task_tts_with_rate() {
        let json_str = run_task_tts(
            "tid-789",
            "cosyvoice-v3-flash",
            TtsParameters {
                voice: "longxiaochun".to_string(),
                format: "mp3".to_string(),
                sample_rate: 22050,
                rate: Some(1.5),
            },
        );
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["payload"]["parameters"]["rate"], 1.5);
    }

    #[test]
    fn continue_task_serialization() {
        let json_str = continue_task("tid-100", "Hello world");
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["header"]["action"], "continue-task");
        assert_eq!(v["header"]["task_id"], "tid-100");
        assert_eq!(v["payload"]["input"]["text"], "Hello world");
    }

    #[test]
    fn finish_task_serialization() {
        let json_str = finish_task("tid-200");
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["header"]["action"], "finish-task");
        assert_eq!(v["header"]["task_id"], "tid-200");
    }

    #[test]
    fn parse_task_started() {
        let json = r#"{"header":{"task_id":"t1","event":"task-started"}}"#;
        let evt = parse_event(json).unwrap();
        assert!(matches!(evt, DashScopeEvent::TaskStarted { task_id } if task_id == "t1"));
    }

    #[test]
    fn parse_task_failed() {
        let json = r#"{"header":{"task_id":"t2","event":"task-failed","code":"InvalidParam","message":"bad model"}}"#;
        let evt = parse_event(json).unwrap();
        match evt {
            DashScopeEvent::TaskFailed {
                task_id,
                code,
                message,
            } => {
                assert_eq!(task_id, "t2");
                assert_eq!(code, "InvalidParam");
                assert_eq!(message, "bad model");
            }
            _ => panic!("expected TaskFailed"),
        }
    }

    #[test]
    fn parse_result_generated_with_asr_sentence() {
        let json = r#"{
            "header":{"task_id":"t3","event":"result-generated"},
            "payload":{"output":{"sentence":{"text":"hello","begin_time":100.0,"end_time":500.0,"sentence_end":true}}}
        }"#;
        let evt = parse_event(json).unwrap();
        let sentence = evt.as_asr_sentence().expect("should have ASR sentence");
        assert_eq!(sentence.text, "hello");
        assert_eq!(sentence.begin_time, 100.0);
        assert_eq!(sentence.end_time, 500.0);
        assert!(sentence.sentence_end);
    }

    #[test]
    fn parse_result_generated_without_asr_payload() {
        // TTS result-generated events may not carry an ASR sentence
        let json = r#"{"header":{"task_id":"t4","event":"result-generated"},"payload":{}}"#;
        let evt = parse_event(json).unwrap();
        assert!(evt.as_asr_sentence().is_none());
    }

    #[test]
    fn parse_task_finished() {
        let json = r#"{"header":{"task_id":"t5","event":"task-finished"}}"#;
        let evt = parse_event(json).unwrap();
        assert!(matches!(evt, DashScopeEvent::TaskFinished { task_id } if task_id == "t5"));
    }

    #[test]
    fn parse_unknown_event() {
        let json = r#"{"header":{"task_id":"t6","event":"something-new"}}"#;
        let evt = parse_event(json).unwrap();
        assert!(
            matches!(evt, DashScopeEvent::Unknown { task_id, event } if task_id == "t6" && event == "something-new")
        );
    }

    #[test]
    fn parse_invalid_json() {
        let result = parse_event("not json at all");
        assert!(result.is_err());
    }
}
