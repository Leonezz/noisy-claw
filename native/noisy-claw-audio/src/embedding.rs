use anyhow::{Context, Result};
use ndarray::{Array1, Array2, ArrayView3, Axis};
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;
use std::sync::Mutex;
use tokenizers::Tokenizer;

/// Sentence embedding inference using ONNX (MiniLM-L12 v2).
///
/// Thread-safe: the ONNX session is wrapped in a Mutex so `embed()`
/// can be called from `spawn_blocking` through an `Arc<SentenceEmbedder>`.
pub struct SentenceEmbedder {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    dim: usize,
}

impl SentenceEmbedder {
    /// Load the ONNX model and tokenizer from disk.
    pub fn new(model_path: &Path, tokenizer_path: &Path) -> Result<Self> {
        let session = Session::builder()
            .context("failed to create ONNX session builder")?
            .with_intra_threads(2)
            .context("failed to set intra threads")?
            .commit_from_file(model_path)
            .context("failed to load ONNX model")?;

        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("failed to load tokenizer: {e}"))?;

        // MiniLM-L12 v2 outputs 384-dimensional embeddings
        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            dim: 384,
        })
    }

    /// Embed a text string into a normalized vector.
    pub fn embed(&self, text: &str) -> Result<Array1<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("tokenization failed: {e}"))?;

        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let attention_mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&m| m as i64)
            .collect();
        let token_type_ids: Vec<i64> = encoding
            .get_type_ids()
            .iter()
            .map(|&t| t as i64)
            .collect();

        let seq_len = input_ids.len();

        let input_ids_array =
            Array2::from_shape_vec((1, seq_len), input_ids).context("input_ids shape")?;
        let attention_mask_array =
            Array2::from_shape_vec((1, seq_len), attention_mask).context("attention_mask shape")?;
        let token_type_ids_array =
            Array2::from_shape_vec((1, seq_len), token_type_ids).context("token_type_ids shape")?;

        let input_ids_tensor = TensorRef::from_array_view(&input_ids_array)?;
        let attention_mask_tensor = TensorRef::from_array_view(&attention_mask_array)?;
        let token_type_ids_tensor = TensorRef::from_array_view(&token_type_ids_array)?;

        let mut session = self.session.lock().map_err(|e| anyhow::anyhow!("session lock poisoned: {e}"))?;
        let outputs = session.run(ort::inputs![
            "input_ids" => input_ids_tensor,
            "attention_mask" => attention_mask_tensor,
            "token_type_ids" => token_type_ids_tensor,
        ])?;

        // Output shape: (1, seq_len, hidden_dim) — mean pool over tokens
        let (shape, raw_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .context("failed to extract output tensor")?;

        // Reconstruct as 3D: [1, seq_len, dim]
        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        let view = ArrayView3::from_shape(
            (dims[0], dims[1], dims[2]),
            raw_data,
        ).context("failed to reshape output")?;

        let tokens_slice = view.index_axis(Axis(0), 0); // [seq_len, dim]

        // Mean pooling over the sequence dimension
        let mean = tokens_slice.mean_axis(Axis(0)).context("mean pooling failed")?;

        // L2 normalize
        let norm = mean.dot(&mean).sqrt();
        if norm > 1e-8 {
            Ok(mean / norm)
        } else {
            Ok(mean)
        }
    }

    /// Cosine similarity between two L2-normalized vectors.
    pub fn cosine_similarity(a: &Array1<f32>, b: &Array1<f32>) -> f32 {
        a.dot(b)
    }

    /// Embedding dimensionality.
    pub fn dim(&self) -> usize {
        self.dim
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::array;

    #[test]
    fn cosine_similarity_identical() {
        let a = array![0.6, 0.8];
        let b = array![0.6, 0.8];
        let sim = SentenceEmbedder::cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = array![1.0, 0.0];
        let b = array![0.0, 1.0];
        let sim = SentenceEmbedder::cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_opposite() {
        let a = array![1.0, 0.0];
        let b = array![-1.0, 0.0];
        let sim = SentenceEmbedder::cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-5);
    }
}
