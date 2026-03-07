pub mod aec;
pub mod audio_utils;
pub mod capture;
pub mod cloud;
pub mod embedding;
pub mod output;
pub mod pipeline;
pub mod playback;
pub mod protocol;
pub mod stt;
pub mod vad;

use std::path::PathBuf;

/// Expand a leading `~` or `~/` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    let home = || std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("~"));
    if path == "~" {
        home()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Resolve the models directory from env var, exe-relative path, or fallback.
pub fn resolve_models_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NOISY_CLAW_MODELS_DIR") {
        let p = PathBuf::from(&dir);
        if p.exists() {
            return p;
        }
        tracing::warn!(
            path = %dir,
            "NOISY_CLAW_MODELS_DIR set but path does not exist, falling back"
        );
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));

    if let Some(ref dir) = exe_dir {
        let models = dir.join("models");
        if models.exists() {
            return models;
        }
    }

    PathBuf::from("models")
}
