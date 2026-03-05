use std::any::Any;
use std::collections::HashMap;

use anyhow::{anyhow, Result};
use tokio::sync::oneshot;

use super::builder::PipelineBuilder;
use super::definition::PipelineDefinition;
use super::node::{NodeHandle, PipelineNode};
use super::types::{NodeSnapshot, PropertyMap};

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
}

/// Snapshot of the entire pipeline for introspection / visualization.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PipelineSnapshot {
    pub name: String,
    pub current_mode: Option<String>,
    pub nodes: HashMap<String, NodeSnapshot>,
}

/// Runtime pipeline: manages node lifecycle, mode switching, introspection.
pub struct Pipeline {
    nodes: HashMap<String, Box<dyn PipelineNode>>,
    handles: HashMap<String, NodeHandle>,
    definition: PipelineDefinition,
    current_mode: Option<String>,
}

impl Pipeline {
    /// Build a pipeline from a JSON definition string.
    pub fn from_json(json: &str) -> Result<Self> {
        let def: PipelineDefinition = serde_json::from_str(json)?;
        Self::from_definition(&def)
    }

    /// Build a pipeline from a parsed definition.
    pub fn from_definition(def: &PipelineDefinition) -> Result<Self> {
        let builder = PipelineBuilder::new(def)?;
        let built = builder.build()?;
        Ok(Self {
            nodes: built.nodes,
            handles: HashMap::new(),
            definition: built.definition,
            current_mode: None,
        })
    }

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

    /// Update a single property on a specific node.
    pub fn set_property(&mut self, node_name: &str, key: &str, value: serde_json::Value) -> Result<()> {
        let node = self.nodes.get_mut(node_name)
            .ok_or_else(|| anyhow!("unknown node: {node_name}"))?;
        let mut map = PropertyMap::new();
        map.insert(key.to_string(), value);
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

    /// Get a mutable reference to a node by name.
    pub fn node_mut(&mut self, name: &str) -> Option<&mut Box<dyn PipelineNode>> {
        self.nodes.get_mut(name)
    }

    /// Downcast a node to a concrete type by name.
    pub fn downcast_node<T: Any>(&self, name: &str) -> Option<&T> {
        self.nodes.get(name)?.as_any().downcast_ref()
    }

    /// Downcast a node to a concrete type by name (mutable).
    pub fn downcast_node_mut<T: Any>(&mut self, name: &str) -> Option<&mut T> {
        self.nodes.get_mut(name)?.as_any_mut().downcast_mut()
    }
}
