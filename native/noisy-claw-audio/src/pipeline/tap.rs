use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

use super::dump::{self, TapMessage};

/// Spawn the WebSocket audio tap server on the given port.
///
/// Clients connect and send a JSON subscription message to select which taps
/// to receive. The server then streams binary audio frames and text VAD metadata.
///
/// Also serves dump directory listings and raw dump files over a simple HTTP-like
/// protocol for the web UI to browse/play recorded audio.
pub fn spawn_server(port: u16, dump_base: Option<PathBuf>) {
    tokio::spawn(async move {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(%e, %port, "tap server: failed to bind");
                return;
            }
        };
        tracing::info!(%port, "tap server: listening");

        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let dump_base = dump_base.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, peer, dump_base).await {
                            tracing::debug!(%e, %peer, "tap server: connection ended");
                        }
                    });
                }
                Err(e) => {
                    tracing::error!(%e, "tap server: accept failed");
                }
            }
        }
    });
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    dump_base: Option<PathBuf>,
) -> anyhow::Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    tracing::info!(%peer, "tap server: client connected");

    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let mut subscriptions: HashSet<String> = HashSet::new();
    let mut subscribe_all = false;

    // Subscribe to the tap broadcast
    let mut tap_rx = dump::tap_subscribe()
        .ok_or_else(|| anyhow::anyhow!("tap not initialized"))?;

    // Diagnostics: count messages forwarded
    let mut audio_msg_count: u64 = 0;
    let mut vad_msg_count: u64 = 0;
    let mut lagged_count: u64 = 0;
    let mut last_stats = tokio::time::Instant::now();

    loop {
        tokio::select! {
            // Client messages (subscription commands, dump requests)
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                            // Subscription: {"subscribe": ["capture", "aec_out"]} or {"subscribe": "*"}
                            if let Some(sub) = val.get("subscribe") {
                                if sub.as_str() == Some("*") {
                                    subscribe_all = true;
                                    tracing::info!(%peer, "tap: subscribed to all taps");
                                } else if let Some(arr) = sub.as_array() {
                                    for item in arr {
                                        if let Some(s) = item.as_str() {
                                            subscriptions.insert(s.to_string());
                                        }
                                    }
                                    tracing::info!(%peer, taps = ?subscriptions, "tap: subscribed");
                                }
                            }

                            // List dump directories: {"list_dumps": true}
                            if val.get("list_dumps").is_some() {
                                let resp = list_dumps(&dump_base);
                                let _ = ws_tx.send(Message::Text(resp.into())).await;
                            }

                            // List files in a dump: {"list_dump_files": "dump_20260304_123456"}
                            if let Some(name) = val.get("list_dump_files").and_then(|v| v.as_str()) {
                                let resp = list_dump_files(&dump_base, name);
                                let _ = ws_tx.send(Message::Text(resp.into())).await;
                            }

                            // Read a dump file: {"read_dump_file": "dump_20260304_123456/capture.pcm", "format": "wav"}
                            if let Some(path) = val.get("read_dump_file").and_then(|v| v.as_str()) {
                                let format = val.get("format").and_then(|v| v.as_str()).unwrap_or("raw");
                                let meta_tap = val.get("tap").and_then(|v| v.as_str());
                                match read_dump_file(&dump_base, path, format, meta_tap) {
                                    Ok(data) => {
                                        let _ = ws_tx.send(Message::Binary(data.into())).await;
                                    }
                                    Err(e) => {
                                        let err = serde_json::json!({"error": e.to_string()});
                                        let _ = ws_tx.send(Message::Text(err.to_string().into())).await;
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        tracing::debug!(%e, %peer, "tap: ws error");
                        break;
                    }
                    _ => {}
                }
            }

            // Tap broadcast messages
            result = tap_rx.recv() => {
                match result {
                    Ok(tap_msg) => {
                        match &tap_msg {
                            TapMessage::Audio { tap, samples, sample_rate, timestamp } => {
                                if subscribe_all || subscriptions.contains(*tap) {
                                    audio_msg_count += 1;
                                    let frame = encode_audio_frame(tap, samples, *sample_rate, *timestamp);
                                    if ws_tx.send(Message::Binary(frame.into())).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            TapMessage::VadMeta { data, timestamp } => {
                                if subscribe_all || subscriptions.contains("vad_meta") {
                                    vad_msg_count += 1;
                                    let json = serde_json::json!({
                                        "type": "vad_meta",
                                        "data": data,
                                        "timestamp": timestamp,
                                    });
                                    if ws_tx.send(Message::Text(json.to_string().into())).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        lagged_count += 1;
                        tracing::warn!(%peer, skipped = n, lagged_count, "tap: subscriber lagged");
                    }
                    Err(_) => break,
                }

                // Periodic diagnostics (every 5 seconds)
                if last_stats.elapsed() >= std::time::Duration::from_secs(5) {
                    tracing::debug!(
                        %peer,
                        audio_msgs = audio_msg_count,
                        vad_msgs = vad_msg_count,
                        lagged = lagged_count,
                        "tap: stats"
                    );
                    last_stats = tokio::time::Instant::now();
                }
            }
        }
    }

    tracing::info!(%peer, "tap server: client disconnected");
    Ok(())
}

/// Encode an audio frame as a binary WebSocket message.
///
/// Format:
/// ```text
/// [1 byte: tap name length]
/// [N bytes: tap name (UTF-8)]
/// [4 bytes: sample_rate u32 LE]
/// [4 bytes: sample_count u32 LE]
/// [8 bytes: timestamp f64 LE]
/// [N*4 bytes: f32 LE samples]
/// ```
fn encode_audio_frame(tap: &str, samples: &[f32], sample_rate: u32, timestamp: f64) -> Vec<u8> {
    let tap_bytes = tap.as_bytes();
    let header_size = 1 + tap_bytes.len() + 4 + 4 + 8;
    let total = header_size + samples.len() * 4;
    let mut buf = Vec::with_capacity(total);

    buf.push(tap_bytes.len() as u8);
    buf.extend_from_slice(tap_bytes);
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    buf.extend_from_slice(&timestamp.to_le_bytes());

    for &s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }

    buf
}

/// List available dump directories.
fn list_dumps(dump_base: &Option<PathBuf>) -> String {
    let Some(base) = dump_base else {
        return serde_json::json!({"dumps": []}).to_string();
    };

    let mut dumps = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    if name.starts_with("dump_") {
                        // Try to read meta.json for details
                        let meta_path = entry.path().join("meta.json");
                        let meta = std::fs::read_to_string(&meta_path).ok();
                        dumps.push(serde_json::json!({
                            "name": name,
                            "meta": meta.and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok()),
                        }));
                    }
                }
            }
        }
    }
    dumps.sort_by(|a, b| {
        b["name"].as_str().unwrap_or("").cmp(&a["name"].as_str().unwrap_or(""))
    });
    serde_json::json!({"dumps": dumps}).to_string()
}

/// List files in a specific dump directory.
fn list_dump_files(dump_base: &Option<PathBuf>, dump_name: &str) -> String {
    let Some(base) = dump_base else {
        return serde_json::json!({"error": "no dump directory configured"}).to_string();
    };

    // Prevent path traversal
    if dump_name.contains("..") || dump_name.contains('/') {
        return serde_json::json!({"error": "invalid dump name"}).to_string();
    }

    let dir = base.join(dump_name);
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push(serde_json::json!({
                    "name": name,
                    "size": size,
                }));
            }
        }
    }
    serde_json::json!({
        "dump": dump_name,
        "files": files,
    }).to_string()
}

/// Read a dump file and optionally convert to WAV format.
fn read_dump_file(
    dump_base: &Option<PathBuf>,
    path: &str,
    format: &str,
    meta_tap: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    let base = dump_base
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no dump directory configured"))?;

    // Prevent path traversal
    if path.contains("..") {
        return Err(anyhow::anyhow!("invalid path"));
    }

    let file_path = base.join(path);
    let raw_data = std::fs::read(&file_path)?;

    if format == "wav" {
        // Determine sample rate from meta.json
        let sample_rate = if let Some(tap_name) = meta_tap {
            // Read meta.json from parent directory
            let parent = file_path.parent().unwrap();
            let meta_path = parent.join("meta.json");
            if let Ok(meta_str) = std::fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&meta_str) {
                    meta["taps"][tap_name]["sample_rate"]
                        .as_u64()
                        .unwrap_or(48000) as u32
                } else {
                    48000
                }
            } else {
                48000
            }
        } else {
            48000
        };

        // Convert raw f32 PCM to WAV
        Ok(raw_f32_to_wav(&raw_data, sample_rate))
    } else {
        Ok(raw_data)
    }
}

/// Convert raw f32 LE PCM data to a WAV file in memory.
fn raw_f32_to_wav(raw: &[u8], sample_rate: u32) -> Vec<u8> {
    let num_samples = raw.len() / 4;
    let num_channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * (num_channels as u32) * (bits_per_sample as u32 / 8);
    let block_align = num_channels * (bits_per_sample / 8);
    let data_size = (num_samples * 2) as u32; // i16 = 2 bytes per sample
    let file_size = 36 + data_size;

    let mut wav = Vec::with_capacity(44 + data_size as usize);

    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    wav.extend_from_slice(&num_channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());

    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());

    // Convert f32 samples to i16
    for i in 0..num_samples {
        let offset = i * 4;
        if offset + 4 <= raw.len() {
            let f = f32::from_le_bytes([raw[offset], raw[offset + 1], raw[offset + 2], raw[offset + 3]]);
            let clamped = f.clamp(-1.0, 1.0);
            let i16_val = (clamped * 32767.0) as i16;
            wav.extend_from_slice(&i16_val.to_le_bytes());
        }
    }

    wav
}
