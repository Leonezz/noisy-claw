use std::collections::HashMap;

use anyhow::{anyhow, Result};
use tokio::sync::{mpsc, oneshot};

use super::builder::PipelineBuilder;
use super::definition::PipelineDefinition;
use super::node::{NodeHandle, PipelineNode};
use super::types::{NodeSnapshot, PortDescriptor, PropertyMap};
use crate::pipeline::FlushSignal;
use crate::protocol::Event;

/// Information about a registered node type (for frontend discovery).
#[derive(Clone, Debug, serde::Serialize)]
pub struct NodeTypeInfo {
    pub node_type: String,
    pub description: String,
    pub ports: Vec<PortDescriptor>,
}

/// Request sent from the tap server (or other introspection clients) to the orchestrator.
pub enum PipelineRequest {
    GetSnapshot {
        reply: oneshot::Sender<PipelineSnapshot>,
    },
    GetDefinition {
        reply: oneshot::Sender<PipelineDefinition>,
    },
    SetProperty {
        node: String,
        key: String,
        value: serde_json::Value,
        reply: oneshot::Sender<Result<()>>,
    },
    SetMode {
        mode: String,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Replace the running pipeline with a new definition (standalone mode).
    LoadPipeline {
        json: String,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Start audio capture on the pipeline (standalone mode).
    StartCapture {
        device: String,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Stop audio capture (standalone mode).
    StopCapture {
        reply: oneshot::Sender<Result<()>>,
    },
    /// Send a command to a specific pipeline node.
    SendCommand {
        node: String,
        cmd: String,
        args: serde_json::Value,
        reply: oneshot::Sender<Result<serde_json::Value>>,
    },
    /// List all registered node types (for pipeline editor).
    GetNodeTypes {
        reply: oneshot::Sender<Vec<NodeTypeInfo>>,
    },
}

/// Snapshot of the entire pipeline for introspection / visualization.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PipelineSnapshot {
    pub name: String,
    pub current_mode: Option<String>,
    pub nodes: HashMap<String, NodeSnapshot>,
}

/// Runtime pipeline: manages node lifecycle, mode switching, introspection.
///
/// The pipeline owns an event bus — all `IpcEvent` output ports are automatically
/// fanned-out to it. The orchestrator reads events via `take_event_rx()` and can
/// inject events (Ready, PlaybackDone, etc.) via `event_tx()`.
pub struct Pipeline {
    nodes: HashMap<String, Box<dyn PipelineNode>>,
    handles: HashMap<String, NodeHandle>,
    definition: PipelineDefinition,
    current_mode: Option<String>,
    event_tx: mpsc::UnboundedSender<Event>,
    event_rx: Option<mpsc::UnboundedReceiver<Event>>,
}

impl Pipeline {
    /// Build a pipeline from a JSON definition string.
    pub fn from_json(json: &str) -> Result<Self> {
        let def: PipelineDefinition = serde_json::from_str(json)?;
        Self::from_definition(&def)
    }

    /// Build a pipeline from a parsed definition.
    pub fn from_definition(def: &PipelineDefinition) -> Result<Self> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let builder = PipelineBuilder::new(def)?;
        let mut built = builder.build(Some(event_tx.clone()))?;

        // Populate port descriptors on the definition from actual node implementations
        for node_def in &mut built.definition.nodes {
            if let Some(node) = built.nodes.get(&node_def.name) {
                node_def.ports = node.ports();
            }
        }

        Ok(Self {
            nodes: built.nodes,
            handles: HashMap::new(),
            definition: built.definition,
            current_mode: None,
            event_tx,
            event_rx: Some(event_rx),
        })
    }

    // ── Event bus ────────────────────────────────────────────────

    /// Take the event receiver. Call once after construction.
    /// All IPC events from nodes + injected events flow through this.
    pub fn take_event_rx(&mut self) -> Option<mpsc::UnboundedReceiver<Event>> {
        self.event_rx.take()
    }

    /// Get a clone of the event sender for injecting events from outside the pipeline.
    pub fn event_tx(&self) -> mpsc::UnboundedSender<Event> {
        self.event_tx.clone()
    }

    // ── Lifecycle ────────────────────────────────────────────────

    /// Start all nodes.
    pub async fn start(&mut self) -> Result<()> {
        for (name, node) in &mut self.nodes {
            let handle = node.start().await?;
            self.handles.insert(name.clone(), handle);
            tracing::info!(%name, "pipeline: node started");
        }
        Ok(())
    }

    /// Shutdown all nodes.
    pub async fn shutdown(&mut self) {
        for (name, node) in &mut self.nodes {
            node.shutdown().await;
            tracing::info!(%name, "pipeline: node shut down");
        }
        for (_, handle) in &mut self.handles {
            handle.shutdown().await;
        }
        self.handles.clear();
    }

    // ── Pipeline-level commands ──────────────────────────────────
    //
    // These handle compound operations that span multiple nodes.
    // The orchestrator calls these instead of knowing which nodes
    // to coordinate. Node-specific commands are dispatched via
    // `send_command()`.

    /// Execute a pipeline-level command.
    pub async fn command(&mut self, cmd: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        match cmd {
            "start_capture" => self.cmd_start_capture(args).await,
            "stop_capture" => self.cmd_stop_capture().await,
            "speak" | "speak_start" | "speak_chunk" | "speak_end" => {
                self.send_command("tts", cmd, args).await
            }
            "stop_speaking" => self.cmd_stop_speaking().await,
            "flush_speak" => self.cmd_flush_speak(args).await,
            "get_status" => self.cmd_get_status().await,
            "set_mode" => {
                let mode = args.get("mode").and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("mode required"))?;
                self.set_mode(mode)?;
                Ok(serde_json::json!({}))
            }
            _ => Err(anyhow!("unknown pipeline command: {cmd}")),
        }
    }

    async fn cmd_start_capture(&mut self, args: serde_json::Value) -> Result<serde_json::Value> {
        // Check VAD initialized
        let vad_ok = self.send_command("vad", "status", serde_json::json!({})).await
            .ok()
            .and_then(|v| v.get("initialized").and_then(|b| b.as_bool()))
            .unwrap_or(false);
        if !vad_ok {
            return Err(anyhow!("VAD not initialized"));
        }

        // Start capture — STT nodes auto-start from their properties in start()
        let device = args.get("device").and_then(|v| v.as_str()).unwrap_or("default");
        let sample_rate = args.get("sample_rate").and_then(|v| v.as_u64())
            .unwrap_or(crate::protocol::PIPELINE_SAMPLE_RATE as u64) as u32;
        self.send_command("capture", "start", serde_json::json!({
            "device": device,
            "sample_rate": sample_rate,
        })).await?;

        Ok(serde_json::json!({}))
    }

    async fn cmd_stop_capture(&mut self) -> Result<serde_json::Value> {
        self.send_command("capture", "stop", serde_json::json!({})).await.ok();
        Ok(serde_json::json!({}))
    }

    async fn cmd_stop_speaking(&mut self) -> Result<serde_json::Value> {
        self.send_command("tts", "stop", serde_json::json!({})).await.ok();
        if let Some(out) = self.nodes.get_mut("output") {
            out.flush(FlushSignal::FlushAll).await;
        }
        Ok(serde_json::json!({}))
    }

    async fn cmd_flush_speak(&mut self, args: serde_json::Value) -> Result<serde_json::Value> {
        let request_id = args.get("request_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(tts) = self.nodes.get_mut("tts") {
            tts.flush(FlushSignal::Flush { request_id: request_id.clone() }).await;
        }
        if let Some(out) = self.nodes.get_mut("output") {
            out.flush(FlushSignal::Flush { request_id }).await;
        }
        Ok(serde_json::json!({}))
    }

    async fn cmd_get_status(&mut self) -> Result<serde_json::Value> {
        let capturing = self.send_command("capture", "status", serde_json::json!({})).await
            .ok()
            .and_then(|v| v.get("capturing").and_then(|b| b.as_bool()))
            .unwrap_or(false);
        let speaking = self.send_command("output", "status", serde_json::json!({})).await
            .ok()
            .and_then(|v| v.get("speaking").and_then(|b| b.as_bool()))
            .unwrap_or(false);
        Ok(serde_json::json!({ "capturing": capturing, "speaking": speaking }))
    }

    // ── Node-level access ────────────────────────────────────────

    /// Apply a named mode (batch property update).
    pub fn set_mode(&mut self, mode_name: &str) -> Result<()> {
        let overrides = self.definition.modes.get(mode_name)
            .ok_or_else(|| anyhow!("unknown mode: {mode_name}"))?
            .clone();

        for (node_name, props) in &overrides {
            if let Some(node) = self.nodes.get_mut(node_name) {
                let map = props.as_object()
                    .cloned()
                    .unwrap_or_default();
                node.update(&map)?;
            }
        }

        self.current_mode = Some(mode_name.to_string());
        tracing::info!(%mode_name, "pipeline: mode applied");
        Ok(())
    }

    /// Send a command to a specific node.
    pub async fn send_command(&mut self, node: &str, cmd: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        let n = self.nodes.get_mut(node)
            .ok_or_else(|| anyhow!("unknown node: {node}"))?;
        n.command(cmd, args).await
    }

    /// Update a single property on a specific node.
    pub fn set_property(&mut self, node_name: &str, key: &str, value: serde_json::Value) -> Result<()> {
        let node = self.nodes.get_mut(node_name)
            .ok_or_else(|| anyhow!("unknown node: {node_name}"))?;
        let mut map = PropertyMap::new();
        map.insert(key.to_string(), value.clone());
        tracing::info!(%node_name, %key, %value, "pipeline: set_property");
        node.update(&map)
    }

    /// Get a snapshot of all nodes for introspection.
    pub fn snapshot(&self) -> PipelineSnapshot {
        let nodes = self.nodes.iter()
            .map(|(name, node)| (name.clone(), node.snapshot()))
            .collect();
        PipelineSnapshot {
            name: self.definition.name.clone(),
            current_mode: self.current_mode.clone(),
            nodes,
        }
    }

    /// Get the pipeline definition.
    pub fn definition(&self) -> &PipelineDefinition {
        &self.definition
    }
}
