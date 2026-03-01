pub mod dashscope_stt;
pub mod dashscope_tts;

use anyhow::{bail, Result};
use super::traits::{SpeechRecognizer, SpeechSynthesizer};

pub fn create_recognizer(model: &str) -> Result<Box<dyn SpeechRecognizer>> {
    match model {
        m if m.starts_with("paraformer") => {
            Ok(Box::new(dashscope_stt::DashScopeRecognizer::new()))
        }
        other => bail!("unknown Aliyun STT model: {other}"),
    }
}

pub fn create_synthesizer(model: &str) -> Result<Box<dyn SpeechSynthesizer>> {
    match model {
        m if m.starts_with("cosyvoice") || m.starts_with("sambert") => {
            Ok(Box::new(dashscope_tts::DashScopeSynthesizer::new()))
        }
        other => bail!("unknown Aliyun TTS model: {other}"),
    }
}
