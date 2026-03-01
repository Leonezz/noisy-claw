pub mod aliyun;
pub mod traits;

use anyhow::{bail, Result};
use traits::{SpeechRecognizer, SpeechSynthesizer};

/// Create STT recognizer for the given provider+model combination.
pub fn create_recognizer(provider: &str, model: &str) -> Result<Box<dyn SpeechRecognizer>> {
    match provider {
        "aliyun" => aliyun::create_recognizer(model),
        other => bail!("unknown STT provider: {other}"),
    }
}

/// Create TTS synthesizer for the given provider+model combination.
pub fn create_synthesizer(provider: &str, model: &str) -> Result<Box<dyn SpeechSynthesizer>> {
    match provider {
        "aliyun" => aliyun::create_synthesizer(model),
        other => bail!("unknown TTS provider: {other}"),
    }
}
