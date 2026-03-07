use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ndarray::Array1;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::embedding::SentenceEmbedder;
use crate::protocol::Event;

pub enum Control {
    /// A final transcript to evaluate for topic shift.
    Transcript { text: String },
    /// VAD state update for silence-based block emission.
    VadState { speaking: bool },
    /// Reconfigure thresholds at runtime.
    Configure {
        similarity_threshold: f32,
        max_block_secs: f64,
        silence_block_secs: f64,
    },
    Shutdown,
}

pub struct Handle {
    pub control_tx: mpsc::Sender<Control>,
    join: JoinHandle<()>,
}

impl Handle {
    pub async fn transcript(&self, text: String) {
        let _ = self.control_tx.send(Control::Transcript { text }).await;
    }

    pub async fn vad_state(&self, speaking: bool) {
        let _ = self.control_tx.send(Control::VadState { speaking }).await;
    }

    pub async fn shutdown(self) {
        let _ = self.control_tx.send(Control::Shutdown).await;
        let _ = self.join.await;
    }
}

/// Spawn the topic detection node.
///
/// The node defers model loading until `enabled_rx` becomes `true`.
/// While disabled, incoming transcripts are dropped.
pub fn spawn(
    event_tx: mpsc::Sender<Event>,
    model_path: PathBuf,
    tokenizer_path: PathBuf,
    similarity_threshold: f32,
    max_block_secs: f64,
    silence_block_secs: f64,
    mut enabled_rx: watch::Receiver<bool>,
) -> Handle {
    let (ctl_tx, mut ctl_rx) = mpsc::channel::<Control>(64);

    let join = tokio::spawn(async move {
        let mut embedder: Option<Arc<SentenceEmbedder>> = None;
        let mut centroid: Option<Array1<f32>> = None;
        let mut topic_start = Instant::now();
        let mut last_speech_time = Instant::now();
        let mut is_speaking = false;
        let mut sim_threshold = similarity_threshold;
        let mut max_block = max_block_secs;
        let mut silence_block = silence_block_secs;

        // EMA alpha for centroid update
        let alpha: f32 = 0.3;

        loop {
            tokio::select! {
                Some(ctl) = ctl_rx.recv() => {
                    match ctl {
                        Control::Transcript { text } => {
                            // Skip if not enabled
                            if !*enabled_rx.borrow() {
                                continue;
                            }

                            // Lazy-load model on first transcript when enabled
                            if embedder.is_none() {
                                tracing::info!("Topic node: loading embedder (lazy)");
                                let mp = model_path.clone();
                                let tp = tokenizer_path.clone();
                                match tokio::task::spawn_blocking(move || {
                                    SentenceEmbedder::new(&mp, &tp)
                                }).await {
                                    Ok(Ok(e)) => {
                                        tracing::info!("Topic node: embedder loaded");
                                        embedder = Some(Arc::new(e));
                                    }
                                    Ok(Err(e)) => {
                                        tracing::error!(%e, "Topic node: failed to load embedder");
                                        let _ = event_tx.send(Event::Error {
                                            message: format!("topic embedder init failed: {e}"),
                                        }).await;
                                        continue;
                                    }
                                    Err(e) => {
                                        tracing::error!(%e, "Topic node: spawn_blocking panicked");
                                        continue;
                                    }
                                }
                            }

                            let emb_ref = embedder.as_ref().unwrap().clone();
                            let embedding = {
                                let text_clone = text.clone();
                                match tokio::task::spawn_blocking(move || {
                                    emb_ref.embed(&text_clone)
                                }).await {
                                    Ok(Ok(emb)) => emb,
                                    Ok(Err(e)) => {
                                        tracing::warn!(%e, "Topic node: embedding failed");
                                        continue;
                                    }
                                    Err(e) => {
                                        tracing::error!(%e, "Topic node: spawn_blocking panicked");
                                        continue;
                                    }
                                }
                            };

                            match centroid {
                                Some(ref mut c) => {
                                    let sim = SentenceEmbedder::cosine_similarity(c, &embedding);
                                    tracing::debug!(
                                        similarity = format!("{sim:.3}"),
                                        threshold = format!("{sim_threshold:.3}"),
                                        "Topic node: similarity"
                                    );

                                    if sim < sim_threshold {
                                        tracing::info!(
                                            similarity = format!("{sim:.3}"),
                                            "Topic node: topic shift detected"
                                        );
                                        let _ = event_tx
                                            .send(Event::TopicShift {
                                                similarity: sim as f64,
                                            })
                                            .await;
                                        // Reset centroid to new topic
                                        *c = embedding;
                                        topic_start = Instant::now();
                                    } else {
                                        // EMA update
                                        *c = c.mapv(|v| v * (1.0 - alpha))
                                            + embedding.mapv(|v| v * alpha);
                                        // Re-normalize
                                        let norm = c.dot(&*c).sqrt();
                                        if norm > 1e-8 {
                                            c.mapv_inplace(|v| v / norm);
                                        }
                                    }
                                }
                                None => {
                                    centroid = Some(embedding);
                                    topic_start = Instant::now();
                                }
                            }

                            last_speech_time = Instant::now();
                        }

                        Control::VadState { speaking } => {
                            if speaking {
                                last_speech_time = Instant::now();
                            }
                            is_speaking = speaking;
                        }

                        Control::Configure {
                            similarity_threshold: st,
                            max_block_secs: mb,
                            silence_block_secs: sb,
                        } => {
                            sim_threshold = st;
                            max_block = mb;
                            silence_block = sb;
                            tracing::info!(
                                sim_threshold,
                                max_block,
                                silence_block,
                                "Topic node: reconfigured"
                            );
                        }

                        Control::Shutdown => {
                            tracing::info!("Topic node: shutdown");
                            break;
                        }
                    }
                }

                // Check time-based block emission every second (only when enabled + model loaded)
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                    if !*enabled_rx.borrow() || centroid.is_none() {
                        continue;
                    }

                    let elapsed = topic_start.elapsed().as_secs_f64();
                    let silence_elapsed = last_speech_time.elapsed().as_secs_f64();

                    // Force emit on max block duration
                    if elapsed > max_block {
                        tracing::info!(
                            elapsed_secs = format!("{elapsed:.1}"),
                            "Topic node: max block duration, forcing emit"
                        );
                        let _ = event_tx
                            .send(Event::TopicShift { similarity: 0.0 })
                            .await;
                        centroid = None;
                        topic_start = Instant::now();
                    }
                    // Force emit on prolonged silence
                    else if !is_speaking && silence_elapsed > silence_block {
                        tracing::info!(
                            silence_secs = format!("{silence_elapsed:.1}"),
                            "Topic node: silence block, forcing emit"
                        );
                        let _ = event_tx
                            .send(Event::TopicShift { similarity: 0.0 })
                            .await;
                        centroid = None;
                        topic_start = Instant::now();
                    }
                }
            }
        }
    });

    Handle {
        control_tx: ctl_tx,
        join,
    }
}

#[cfg(test)]
mod tests {
    // Integration tests require model files; unit logic tested via embedding::tests
}
