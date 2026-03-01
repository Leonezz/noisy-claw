//! Integration tests for the noisy-claw-audio IPC protocol.
//!
//! These tests spawn the binary and communicate via JSON-over-stdio,
//! verifying the full command/event pipeline without requiring audio
//! hardware or ML models.

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

/// Spawn the binary and return (child, stdin, stdout reader).
fn spawn_audio_process() -> (
    std::process::Child,
    std::process::ChildStdin,
    BufReader<std::process::ChildStdout>,
) {
    let bin = env!("CARGO_BIN_EXE_noisy-claw-audio");
    let child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn noisy-claw-audio");

    let mut child = child;
    let stdin = child.stdin.take().expect("no stdin");
    let stdout = BufReader::new(child.stdout.take().expect("no stdout"));
    (child, stdin, stdout)
}

/// Read one JSON line from stdout, with a timeout.
fn read_event(reader: &mut BufReader<std::process::ChildStdout>) -> Value {
    // Use a simple blocking read — the binary should respond quickly
    let mut line = String::new();
    reader.read_line(&mut line).expect("failed to read line");
    serde_json::from_str(line.trim()).expect("invalid JSON from binary")
}

/// Send a JSON command to the binary.
fn send_command(stdin: &mut std::process::ChildStdin, cmd: &Value) {
    let json = serde_json::to_string(cmd).unwrap();
    writeln!(stdin, "{}", json).expect("failed to write to stdin");
    stdin.flush().expect("failed to flush stdin");
}

#[test]
fn ready_event_on_startup() {
    let (mut child, _stdin, mut stdout) = spawn_audio_process();
    let event = read_event(&mut stdout);

    assert_eq!(event["event"], "ready");

    // Clean up
    drop(_stdin); // closes stdin → triggers EOF → binary exits
    let _ = child.wait();
}

#[test]
fn get_status_when_idle() {
    let (mut child, mut stdin, mut stdout) = spawn_audio_process();

    // Consume the ready event
    let ready = read_event(&mut stdout);
    assert_eq!(ready["event"], "ready");

    // Send get_status
    send_command(
        &mut stdin,
        &serde_json::json!({"cmd": "get_status"}),
    );

    let status = read_event(&mut stdout);
    assert_eq!(status["event"], "status");
    assert_eq!(status["capturing"], false);
    assert_eq!(status["playing"], false);

    // Shutdown
    send_command(&mut stdin, &serde_json::json!({"cmd": "shutdown"}));
    let exit = child.wait().expect("wait failed");
    assert!(exit.success());
}

#[test]
fn shutdown_exits_cleanly() {
    let (mut child, mut stdin, mut stdout) = spawn_audio_process();

    let ready = read_event(&mut stdout);
    assert_eq!(ready["event"], "ready");

    send_command(&mut stdin, &serde_json::json!({"cmd": "shutdown"}));

    let exit = child.wait().expect("wait failed");
    assert!(exit.success());
}

#[test]
fn invalid_json_returns_error() {
    let (mut child, mut stdin, mut stdout) = spawn_audio_process();

    let ready = read_event(&mut stdout);
    assert_eq!(ready["event"], "ready");

    // Send garbage JSON
    writeln!(stdin, "not valid json").unwrap();
    stdin.flush().unwrap();

    let error = read_event(&mut stdout);
    assert_eq!(error["event"], "error");
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("invalid command"));

    send_command(&mut stdin, &serde_json::json!({"cmd": "shutdown"}));
    let exit = child.wait().expect("wait failed");
    assert!(exit.success());
}

#[test]
fn unknown_command_returns_error() {
    let (mut child, mut stdin, mut stdout) = spawn_audio_process();

    let ready = read_event(&mut stdout);
    assert_eq!(ready["event"], "ready");

    // Send a valid JSON but unknown command
    send_command(
        &mut stdin,
        &serde_json::json!({"cmd": "unknown_command"}),
    );

    let error = read_event(&mut stdout);
    assert_eq!(error["event"], "error");

    send_command(&mut stdin, &serde_json::json!({"cmd": "shutdown"}));
    let exit = child.wait().expect("wait failed");
    assert!(exit.success());
}

#[test]
fn start_capture_without_models_returns_error() {
    let (mut child, mut stdin, mut stdout) = spawn_audio_process();

    let ready = read_event(&mut stdout);
    assert_eq!(ready["event"], "ready");

    // start_capture will fail because the VAD model file doesn't exist
    send_command(
        &mut stdin,
        &serde_json::json!({"cmd": "start_capture"}),
    );

    let error = read_event(&mut stdout);
    assert_eq!(error["event"], "error");
    // Should mention VAD init failure
    let msg = error["message"].as_str().unwrap();
    assert!(
        msg.contains("VAD") || msg.contains("init"),
        "unexpected error message: {msg}"
    );

    send_command(&mut stdin, &serde_json::json!({"cmd": "shutdown"}));
    let exit = child.wait().expect("wait failed");
    assert!(exit.success());
}

#[test]
fn stop_capture_when_idle_is_noop() {
    let (mut child, mut stdin, mut stdout) = spawn_audio_process();

    let ready = read_event(&mut stdout);
    assert_eq!(ready["event"], "ready");

    // stop_capture when nothing is capturing — should not error
    send_command(
        &mut stdin,
        &serde_json::json!({"cmd": "stop_capture"}),
    );

    // Verify we can still get status after
    send_command(
        &mut stdin,
        &serde_json::json!({"cmd": "get_status"}),
    );

    let status = read_event(&mut stdout);
    assert_eq!(status["event"], "status");
    assert_eq!(status["capturing"], false);

    send_command(&mut stdin, &serde_json::json!({"cmd": "shutdown"}));
    let exit = child.wait().expect("wait failed");
    assert!(exit.success());
}

#[test]
fn play_audio_nonexistent_file_returns_error() {
    let (mut child, mut stdin, mut stdout) = spawn_audio_process();

    let ready = read_event(&mut stdout);
    assert_eq!(ready["event"], "ready");

    send_command(
        &mut stdin,
        &serde_json::json!({"cmd": "play_audio", "path": "/tmp/__nonexistent_audio_file__.mp3"}),
    );

    let event = read_event(&mut stdout);
    assert_eq!(event["event"], "error");

    send_command(&mut stdin, &serde_json::json!({"cmd": "shutdown"}));
    let exit = child.wait().expect("wait failed");
    assert!(exit.success());
}

#[test]
fn eof_stdin_exits_cleanly() {
    let (mut child, stdin, mut stdout) = spawn_audio_process();

    let ready = read_event(&mut stdout);
    assert_eq!(ready["event"], "ready");

    // Close stdin — binary should exit on EOF
    drop(stdin);

    let exit = child.wait().expect("wait failed");
    assert!(exit.success());
}

#[test]
fn multiple_get_status_calls() {
    let (mut child, mut stdin, mut stdout) = spawn_audio_process();

    let ready = read_event(&mut stdout);
    assert_eq!(ready["event"], "ready");

    // Send multiple status requests
    for _ in 0..3 {
        send_command(
            &mut stdin,
            &serde_json::json!({"cmd": "get_status"}),
        );
        let status = read_event(&mut stdout);
        assert_eq!(status["event"], "status");
    }

    send_command(&mut stdin, &serde_json::json!({"cmd": "shutdown"}));
    let exit = child.wait().expect("wait failed");
    assert!(exit.success());
}
