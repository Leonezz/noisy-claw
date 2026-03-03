use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::capture::AudioCapture;

use super::AudioFrame;

pub enum Control {
    Start { device: String, sample_rate: u32 },
    Stop,
    Shutdown,
}

pub struct Handle {
    pub control_tx: mpsc::Sender<Control>,
    capturing: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

impl Handle {
    pub async fn start(&self, device: &str, sample_rate: u32) {
        let _ = self
            .control_tx
            .send(Control::Start {
                device: device.to_string(),
                sample_rate,
            })
            .await;
    }

    pub async fn stop(&self) {
        let _ = self.control_tx.send(Control::Stop).await;
    }

    pub async fn shutdown(self) {
        let _ = self.control_tx.send(Control::Shutdown).await;
        let _ = self.join.await;
    }

    /// Whether the microphone is actively capturing audio.
    pub fn is_capturing(&self) -> bool {
        self.capturing.load(Ordering::SeqCst)
    }
}

pub fn spawn(audio_tx: mpsc::UnboundedSender<AudioFrame>) -> Handle {
    let (ctl_tx, mut ctl_rx) = mpsc::channel(16);
    let capturing = Arc::new(AtomicBool::new(false));
    let capturing_flag = capturing.clone();

    let join = tokio::spawn(async move {
        let mut capture = AudioCapture::new();
        let mut frame_rx: Option<mpsc::UnboundedReceiver<Vec<f32>>> = None;
        let mut current_sample_rate: u32 = 16000;
        tracing::info!("capture node: task started");

        loop {
            tokio::select! {
                Some(ctl) = ctl_rx.recv() => {
                    match ctl {
                        Control::Start { device, sample_rate } => {
                            tracing::info!(%device, sample_rate, "capture node: starting mic");
                            current_sample_rate = sample_rate;
                            match capture.start(&device, sample_rate) {
                                Ok(rx) => {
                                    frame_rx = Some(rx);
                                    capturing_flag.store(true, Ordering::SeqCst);
                                    tracing::info!(%device, sample_rate, "capture node: mic started successfully");
                                }
                                Err(e) => {
                                    tracing::error!(%e, "capture node: start failed");
                                }
                            }
                        }
                        Control::Stop => {
                            capture.stop();
                            frame_rx = None;
                            capturing_flag.store(false, Ordering::SeqCst);
                            tracing::info!("capture node: stopped");
                        }
                        Control::Shutdown => {
                            capture.stop();
                            capturing_flag.store(false, Ordering::SeqCst);
                            tracing::info!("capture node: shutdown");
                            break;
                        }
                    }
                }

                Some(samples) = async {
                    match frame_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let _ = audio_tx.send(AudioFrame {
                        samples,
                        sample_rate: current_sample_rate,
                        vad: None,
                    });
                }
            }
        }
    });

    Handle {
        control_tx: ctl_tx,
        capturing,
        join,
    }
}
