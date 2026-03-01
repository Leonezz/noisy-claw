use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    StartCapture {
        #[serde(default = "default_device")]
        device: String,
        #[serde(default = "default_sample_rate")]
        sample_rate: u32,
    },
    StopCapture,
    PlayAudio {
        path: String,
    },
    StopPlayback,
    GetStatus,
    Shutdown,
}

fn default_device() -> String {
    "default".to_string()
}

fn default_sample_rate() -> u32 {
    16000
}

#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Ready,
    Vad {
        speaking: bool,
    },
    Transcript {
        text: String,
        is_final: bool,
        start: f64,
        end: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        confidence: Option<f64>,
    },
    PlaybackDone,
    Status {
        capturing: bool,
        playing: bool,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Command deserialization ---

    #[test]
    fn deserialize_start_capture_defaults() {
        let json = r#"{"cmd":"start_capture"}"#;
        let cmd: Command = serde_json::from_str(json).unwrap();
        match cmd {
            Command::StartCapture { device, sample_rate } => {
                assert_eq!(device, "default");
                assert_eq!(sample_rate, 16000);
            }
            _ => panic!("expected StartCapture"),
        }
    }

    #[test]
    fn deserialize_start_capture_with_options() {
        let json = r#"{"cmd":"start_capture","device":"MacBook Pro Microphone","sample_rate":44100}"#;
        let cmd: Command = serde_json::from_str(json).unwrap();
        match cmd {
            Command::StartCapture { device, sample_rate } => {
                assert_eq!(device, "MacBook Pro Microphone");
                assert_eq!(sample_rate, 44100);
            }
            _ => panic!("expected StartCapture"),
        }
    }

    #[test]
    fn deserialize_stop_capture() {
        let json = r#"{"cmd":"stop_capture"}"#;
        let cmd: Command = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, Command::StopCapture));
    }

    #[test]
    fn deserialize_play_audio() {
        let json = r#"{"cmd":"play_audio","path":"/tmp/audio.mp3"}"#;
        let cmd: Command = serde_json::from_str(json).unwrap();
        match cmd {
            Command::PlayAudio { path } => assert_eq!(path, "/tmp/audio.mp3"),
            _ => panic!("expected PlayAudio"),
        }
    }

    #[test]
    fn deserialize_stop_playback() {
        let json = r#"{"cmd":"stop_playback"}"#;
        let cmd: Command = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, Command::StopPlayback));
    }

    #[test]
    fn deserialize_get_status() {
        let json = r#"{"cmd":"get_status"}"#;
        let cmd: Command = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, Command::GetStatus));
    }

    #[test]
    fn deserialize_shutdown() {
        let json = r#"{"cmd":"shutdown"}"#;
        let cmd: Command = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, Command::Shutdown));
    }

    #[test]
    fn deserialize_invalid_command() {
        let json = r#"{"cmd":"unknown"}"#;
        assert!(serde_json::from_str::<Command>(json).is_err());
    }

    // --- Event serialization ---

    #[test]
    fn serialize_ready() {
        let json = serde_json::to_string(&Event::Ready).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["event"], "ready");
    }

    #[test]
    fn serialize_vad() {
        let json = serde_json::to_string(&Event::Vad { speaking: true }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["event"], "vad");
        assert_eq!(v["speaking"], true);
    }

    #[test]
    fn serialize_transcript_without_confidence() {
        let event = Event::Transcript {
            text: "hello world".to_string(),
            is_final: true,
            start: 0.0,
            end: 1.2,
            confidence: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["event"], "transcript");
        assert_eq!(v["text"], "hello world");
        assert_eq!(v["is_final"], true);
        assert!(v.get("confidence").is_none());
    }

    #[test]
    fn serialize_transcript_with_confidence() {
        let event = Event::Transcript {
            text: "hi".to_string(),
            is_final: true,
            start: 0.0,
            end: 0.5,
            confidence: Some(0.95),
        };
        let json = serde_json::to_string(&event).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["confidence"], 0.95);
    }

    #[test]
    fn serialize_playback_done() {
        let json = serde_json::to_string(&Event::PlaybackDone).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["event"], "playback_done");
    }

    #[test]
    fn serialize_status() {
        let event = Event::Status { capturing: true, playing: false };
        let json = serde_json::to_string(&event).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["event"], "status");
        assert_eq!(v["capturing"], true);
        assert_eq!(v["playing"], false);
    }

    #[test]
    fn serialize_error() {
        let event = Event::Error { message: "device not found".to_string() };
        let json = serde_json::to_string(&event).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["event"], "error");
        assert_eq!(v["message"], "device not found");
    }

    // --- Round-trip: TS serializes command → Rust deserializes ---

    #[test]
    fn round_trip_all_commands() {
        let commands = vec![
            r#"{"cmd":"start_capture"}"#,
            r#"{"cmd":"stop_capture"}"#,
            r#"{"cmd":"play_audio","path":"/tmp/test.mp3"}"#,
            r#"{"cmd":"stop_playback"}"#,
            r#"{"cmd":"get_status"}"#,
            r#"{"cmd":"shutdown"}"#,
        ];
        for json in commands {
            assert!(serde_json::from_str::<Command>(json).is_ok(), "failed to parse: {json}");
        }
    }

    // --- Round-trip: Rust serializes event → TS would parse ---

    #[test]
    fn all_events_produce_valid_json_with_event_field() {
        let events = vec![
            Event::Ready,
            Event::Vad { speaking: false },
            Event::Transcript {
                text: "test".to_string(),
                is_final: false,
                start: 0.0,
                end: 1.0,
                confidence: None,
            },
            Event::PlaybackDone,
            Event::Status { capturing: false, playing: true },
            Event::Error { message: "fail".to_string() },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert!(v.get("event").is_some(), "missing 'event' field in: {json}");
        }
    }
}
