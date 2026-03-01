use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct RecognitionResult {
    pub text: String,
    pub is_final: bool,
    pub start_time: f64,
    pub end_time: f64,
    pub confidence: Option<f64>,
}

/// Config for a speech recognizer session.
/// The trait implementation reads only the fields it needs;
/// unused fields (e.g. `extra`) are for provider-specific tuning.
#[derive(Debug, Clone, Deserialize)]
pub struct RecognizerConfig {
    pub api_key: String,
    pub endpoint: Option<String>,
    pub model: String,
    pub languages: Vec<String>,
    pub sample_rate: u32,
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

/// Streaming STT — lifecycle: start -> feed_audio* / poll_result* -> stop
#[async_trait]
pub trait SpeechRecognizer: Send {
    async fn start(&mut self, config: &RecognizerConfig) -> Result<()>;
    async fn feed_audio(&mut self, samples: &[f32]) -> Result<()>;
    async fn poll_result(&mut self) -> Result<Option<RecognitionResult>>;
    async fn stop(&mut self) -> Result<()>;
}

#[derive(Debug, Clone, Deserialize)]
pub struct SynthesizerConfig {
    pub api_key: String,
    pub endpoint: Option<String>,
    pub model: String,
    pub voice: String,
    pub format: String,
    pub sample_rate: u32,
    pub speed: Option<f64>,
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

/// TTS — takes text, produces a playable audio file.
#[async_trait]
pub trait SpeechSynthesizer: Send {
    async fn synthesize(&self, text: &str, config: &SynthesizerConfig) -> Result<PathBuf>;
}
