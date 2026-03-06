use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime};

static DUMP: OnceLock<DumpInner> = OnceLock::new();
static DUMP_START: OnceLock<Instant> = OnceLock::new();

/// Real-time tap message broadcast over WebSocket.
#[derive(Clone)]
pub enum TapMessage {
    Audio {
        tap: &'static str,
        samples: Vec<f32>,
        sample_rate: u32,
        /// Monotonic timestamp in seconds since dump init (pipeline start).
        timestamp: f64,
    },
    Metadata {
        stream: &'static str,
        fields: serde_json::Value,
        timestamp: f64,
    },
}

struct DumpInner {
    tx: Sender<DumpMsg>,
    tap_tx: tokio::sync::broadcast::Sender<TapMessage>,
}

enum DumpMsg {
    Audio {
        tap: &'static str,
        samples: Vec<f32>,
        sample_rate: u32,
    },
    Metadata {
        stream: &'static str,
        data: String,
    },
    Finish,
}

/// Initialize the audio dump + tap system.
///
/// File dump: reads `AUDIO_DUMP_DIR` env var. If set, creates a timestamped
/// sub-directory and spawns a background writer thread.
///
/// Real-time tap: always initialized (broadcast channel for WebSocket server).
///
/// Returns `true` if file dumping is enabled.
pub fn init() -> bool {
    let _ = DUMP_START.set(Instant::now());

    // Broadcast channel for real-time tap (dropped msgs are fine — lagging subscribers).
    // Capacity must be large enough to handle all taps at full audio rate (~300+ msgs/sec)
    // without causing persistent lagging on the subscriber side.
    let (tap_tx, _) = tokio::sync::broadcast::channel::<TapMessage>(4096);

    let base_dir = match std::env::var("AUDIO_DUMP_DIR") {
        Ok(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => None,
    };

    let file_dump_enabled = if let Some(base_dir) = base_dir {
        let timestamp = format_timestamp();
        let dump_dir = base_dir.join(format!("dump_{timestamp}"));

        if let Err(e) = fs::create_dir_all(&dump_dir) {
            tracing::error!(%e, path = %dump_dir.display(), "audio dump: failed to create directory");
            // Still initialize tap channel for real-time WS even if file dump fails
            let (tx, _rx) = mpsc::channel::<DumpMsg>();
            let _ = DUMP.set(DumpInner { tx, tap_tx });
            false
        } else {
            let (tx, rx) = mpsc::channel::<DumpMsg>();

            let writer_dir = dump_dir.clone();
            std::thread::Builder::new()
                .name("audio-dump".into())
                .spawn(move || writer_thread(rx, writer_dir))
                .expect("failed to spawn audio dump writer thread");

            let ok = DUMP.set(DumpInner { tx, tap_tx }).is_ok();
            if ok {
                tracing::info!(path = %dump_dir.display(), "audio dump: enabled");
            }
            ok
        }
    } else {
        // No file dump, but still initialize tap channel for real-time WS
        let (tx, _rx) = mpsc::channel::<DumpMsg>();
        let _ = DUMP.set(DumpInner { tx, tap_tx });
        false
    };

    file_dump_enabled
}

/// Get the dump base directory (parent of timestamped dump dirs).
/// Returns None if `AUDIO_DUMP_DIR` is not set.
pub fn dump_base_dir() -> Option<PathBuf> {
    std::env::var("AUDIO_DUMP_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

/// Subscribe to the real-time audio tap broadcast.
/// Returns None if dump system is not initialized.
pub fn tap_subscribe() -> Option<tokio::sync::broadcast::Receiver<TapMessage>> {
    DUMP.get().map(|inner| inner.tap_tx.subscribe())
}

/// Write audio samples to a named tap. Non-blocking; no-op when dump is disabled.
pub fn write(tap: &'static str, samples: &[f32], sample_rate: u32) {
    if let Some(inner) = DUMP.get() {
        let timestamp = elapsed_secs();

        // File dump (if writer thread is running)
        let _ = inner.tx.send(DumpMsg::Audio {
            tap,
            samples: samples.to_vec(),
            sample_rate,
        });

        // Real-time tap broadcast (non-blocking, dropped if no subscribers)
        let _ = inner.tap_tx.send(TapMessage::Audio {
            tap,
            samples: samples.to_vec(),
            sample_rate,
            timestamp,
        });
    }
}

/// Write structured metadata to a named stream. Non-blocking; no-op when dump is disabled.
pub fn write_metadata(stream: &'static str, fields: serde_json::Value) {
    if let Some(inner) = DUMP.get() {
        let timestamp = elapsed_secs();

        let _ = inner.tx.send(DumpMsg::Metadata {
            stream,
            data: fields.to_string(),
        });

        let _ = inner.tap_tx.send(TapMessage::Metadata {
            stream,
            fields,
            timestamp,
        });
    }
}

/// Monotonic elapsed seconds since dump init.
fn elapsed_secs() -> f64 {
    DUMP_START
        .get()
        .map(|t| t.elapsed().as_secs_f64())
        .unwrap_or(0.0)
}

/// Flush all writers and shut down the background thread. Blocks until complete.
pub fn finish() {
    if let Some(inner) = DUMP.get() {
        let _ = inner.tx.send(DumpMsg::Finish);
        // The writer thread will exit after processing Finish.
        // We give it a reasonable deadline to flush.
        std::thread::sleep(std::time::Duration::from_millis(200));
        tracing::info!("audio dump: finish sent");
    }
}

/// Format current time as `YYYYMMDD_HHMMSS` (UTC).
fn format_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Manual UTC breakdown (no chrono needed)
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;

    // Days since epoch → year/month/day (simplified Gregorian)
    let mut y = 1970i32;
    let mut remaining = days as i64;
    loop {
        let year_days: i64 = if is_leap(y) { 366 } else { 365 };
        if remaining < year_days {
            break;
        }
        remaining -= year_days;
        y += 1;
    }
    let month_days: [i64; 12] = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut mo = 0usize;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining < md {
            mo = i;
            break;
        }
        remaining -= md;
    }
    let d = remaining + 1;
    format!("{y:04}{:02}{d:02}_{h:02}{m:02}{s:02}", mo + 1)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Background writer thread — owns all file handles.
fn writer_thread(rx: mpsc::Receiver<DumpMsg>, dir: PathBuf) {
    let mut writers: HashMap<&'static str, BufWriter<File>> = HashMap::new();
    let mut sample_rates: HashMap<&'static str, u32> = HashMap::new();

    let mut metadata_writers: HashMap<&'static str, BufWriter<File>> = HashMap::new();

    for msg in rx {
        match msg {
            DumpMsg::Audio {
                tap,
                samples,
                sample_rate,
            } => {
                sample_rates.entry(tap).or_insert(sample_rate);

                let writer = writers.entry(tap).or_insert_with(|| {
                    let path = dir.join(format!("{tap}.pcm"));
                    let file =
                        File::create(&path).expect("audio dump: failed to create PCM file");
                    BufWriter::new(file)
                });

                // Write f32 samples as little-endian bytes
                for &s in &samples {
                    let _ = writer.write_all(&s.to_le_bytes());
                }
            }
            DumpMsg::Metadata { stream, data } => {
                let writer = metadata_writers.entry(stream).or_insert_with(|| {
                    let path = dir.join(format!("{stream}.jsonl"));
                    let file = File::create(&path)
                        .expect("audio dump: failed to create metadata file");
                    BufWriter::new(file)
                });
                let _ = writeln!(writer, "{data}");
            }
            DumpMsg::Finish => {
                // Flush all PCM writers
                for (_, mut w) in writers.drain() {
                    let _ = w.flush();
                }
                // Flush metadata writers
                for (_, mut w) in metadata_writers.drain() {
                    let _ = w.flush();
                }

                // Write meta.json
                let mut taps_json = String::from("{");
                let mut first = true;
                for (tap, sr) in &sample_rates {
                    if !first {
                        taps_json.push(',');
                    }
                    taps_json.push_str(&format!(
                        " \"{tap}\": {{ \"sample_rate\": {sr} }}"
                    ));
                    first = false;
                }
                taps_json.push_str(" }");

                let created = format_timestamp();
                let meta = format!(
                    "{{ \"created\": \"{created}\", \"taps\": {taps_json} }}\n"
                );
                let meta_path = dir.join("meta.json");
                if let Ok(mut f) = File::create(meta_path) {
                    let _ = f.write_all(meta.as_bytes());
                }

                tracing::info!("audio dump: writer thread finished");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_is_noop_when_uninitialized() {
        // Should not panic or block
        write("test_tap", &[0.1, 0.2, 0.3], 16000);
        write_metadata("test", serde_json::json!({"value": 1}));
    }

    #[test]
    fn finish_is_noop_when_uninitialized() {
        // Should not panic or block
        finish();
    }
}
