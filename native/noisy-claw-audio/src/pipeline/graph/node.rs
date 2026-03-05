use anyhow::Result;
use tokio::sync::mpsc;

use super::types::{NodeSnapshot, PortDescriptor, PropertyDescriptor, PropertyMap};
use super::wiring::NodeWiring;
use crate::pipeline::{FlushAck, FlushSignal};

/// Opaque handle returned by PipelineNode::start().
/// The pipeline uses this for lifecycle management.
pub struct NodeHandle {
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl NodeHandle {
    pub fn new(shutdown_tx: mpsc::Sender<()>) -> Self {
        Self {
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// Signal the node to shut down.
    pub async fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
    }
}

/// Core trait for all composable pipeline nodes.
///
/// Nodes self-register via `inventory::submit!` with a `NodeFactoryEntry`.
/// The pipeline builder creates nodes, wires their ports via `NodeWiring`,
/// then calls `start()` which returns a `NodeHandle`.
#[async_trait::async_trait]
pub trait PipelineNode: NodeWiring + Send + 'static {
    /// Unique type identifier (e.g., "capture", "vad", "stt").
    fn node_type(&self) -> &'static str;

    /// Declare this node's input and output ports.
    fn ports(&self) -> Vec<PortDescriptor>;

    /// Declare configurable properties with types and defaults.
    fn property_descriptors(&self) -> Vec<PropertyDescriptor>;

    /// Apply property updates. Called on mode switch, user tuning, etc.
    fn update(&mut self, properties: &PropertyMap) -> Result<()>;

    /// Get current state snapshot for introspection / visualization.
    fn snapshot(&self) -> NodeSnapshot;

    /// Spawn the node's internal task/thread. Returns a handle for shutdown.
    /// The node decides its own execution model internally.
    async fn start(&mut self) -> Result<NodeHandle>;

    /// Flush buffered data.
    async fn flush(&mut self, signal: FlushSignal) -> FlushAck;

    /// Graceful shutdown — stop processing and release resources.
    async fn shutdown(&mut self);
}
