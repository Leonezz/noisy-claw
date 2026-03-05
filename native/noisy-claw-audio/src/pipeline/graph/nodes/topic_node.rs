use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use tokio::sync::mpsc;

use crate::pipeline::graph::node::{NodeHandle, PipelineNode};
use crate::pipeline::graph::registry::NodeFactoryEntry;
use crate::pipeline::graph::types::{
    Direction, NodeSnapshot, NodeStatus, PortDescriptor, PortType,
    PropType, PropertyDescriptor, PropertyMap,
};
use crate::pipeline::graph::wiring::{InputEndpoint, NodeWiring, OutputEndpoint};
use crate::pipeline::{topic, FlushAck, FlushSignal, NodeId};
use crate::protocol::Event;

pub struct TopicNode {
    model_path: String,
    tokenizer_path: String,
    similarity_threshold: f32,
    max_block_secs: f64,
    silence_block_secs: f64,
    ipc_event_out: Option<OutputEndpoint>,
    inner: Option<topic::Handle>,
    status: NodeStatus,
}

impl TopicNode {
    pub fn new(props: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            model_path: props.get("model_path").and_then(|v| v.as_str())
                .unwrap_or("").to_string(),
            tokenizer_path: props.get("tokenizer_path").and_then(|v| v.as_str())
                .unwrap_or("").to_string(),
            similarity_threshold: props.get("similarity_threshold").and_then(|v| v.as_f64())
                .unwrap_or(0.65) as f32,
            max_block_secs: props.get("max_block_secs").and_then(|v| v.as_f64())
                .unwrap_or(120.0),
            silence_block_secs: props.get("silence_block_secs").and_then(|v| v.as_f64())
                .unwrap_or(30.0),
            ipc_event_out: None,
            inner: None,
            status: NodeStatus::Created,
        })
    }

    pub fn inner(&self) -> Option<&topic::Handle> { self.inner.as_ref() }
}

impl NodeWiring for TopicNode {
    fn accept_input(&mut self, port: &str, _ep: InputEndpoint) -> Result<()> {
        Err(anyhow!("topic: no input port '{port}'"))
    }

    fn set_output(&mut self, port: &str, ep: OutputEndpoint) -> Result<()> {
        match port {
            "ipc_event_out" => { self.ipc_event_out = Some(ep); Ok(()) }
            _ => Err(anyhow!("topic: unknown output '{port}'")),
        }
    }
}

#[async_trait::async_trait]
impl PipelineNode for TopicNode {
    fn node_type(&self) -> &'static str { "topic" }

    fn ports(&self) -> Vec<PortDescriptor> { topic_ports() }

    fn property_descriptors(&self) -> Vec<PropertyDescriptor> {
        vec![
            PropertyDescriptor {
                name: Cow::Borrowed("model_path"),
                value_type: PropType::String,
                default: serde_json::json!(""),
                description: "Path to sentence embedding model",
            },
            PropertyDescriptor {
                name: Cow::Borrowed("tokenizer_path"),
                value_type: PropType::String,
                default: serde_json::json!(""),
                description: "Path to tokenizer",
            },
            PropertyDescriptor {
                name: Cow::Borrowed("similarity_threshold"),
                value_type: PropType::Float { min: 0.0, max: 1.0 },
                default: serde_json::json!(0.65),
                description: "Cosine similarity threshold for topic shift",
            },
            PropertyDescriptor {
                name: Cow::Borrowed("max_block_secs"),
                value_type: PropType::Float { min: 10.0, max: 600.0 },
                default: serde_json::json!(120.0),
                description: "Max seconds before forced topic emit",
            },
            PropertyDescriptor {
                name: Cow::Borrowed("silence_block_secs"),
                value_type: PropType::Float { min: 5.0, max: 120.0 },
                default: serde_json::json!(30.0),
                description: "Silence seconds before forced topic emit",
            },
        ]
    }

    fn update(&mut self, props: &PropertyMap) -> Result<()> {
        let mut reconfigure = false;
        if let Some(v) = props.get("similarity_threshold") {
            if let Some(t) = v.as_f64() { self.similarity_threshold = t as f32; reconfigure = true; }
        }
        if let Some(v) = props.get("max_block_secs") {
            if let Some(t) = v.as_f64() { self.max_block_secs = t; reconfigure = true; }
        }
        if let Some(v) = props.get("silence_block_secs") {
            if let Some(t) = v.as_f64() { self.silence_block_secs = t; reconfigure = true; }
        }
        if reconfigure {
            if let Some(ref h) = self.inner {
                let _ = h.control_tx.try_send(topic::Control::Configure {
                    similarity_threshold: self.similarity_threshold,
                    max_block_secs: self.max_block_secs,
                    silence_block_secs: self.silence_block_secs,
                });
            }
        }
        Ok(())
    }

    fn snapshot(&self) -> NodeSnapshot {
        let mut p = serde_json::Map::new();
        p.insert("model_path".into(), serde_json::json!(self.model_path));
        p.insert("tokenizer_path".into(), serde_json::json!(self.tokenizer_path));
        p.insert("similarity_threshold".into(), serde_json::json!(self.similarity_threshold));
        p.insert("max_block_secs".into(), serde_json::json!(self.max_block_secs));
        p.insert("silence_block_secs".into(), serde_json::json!(self.silence_block_secs));
        NodeSnapshot {
            node_type: "topic".to_string(),
            status: self.status.clone(),
            properties: p,
            metrics: HashMap::new(),
        }
    }

    async fn start(&mut self) -> Result<NodeHandle> {
        // IpcEvent output: bounded → PortSender bridge
        let ipc_out = match self.ipc_event_out.take() {
            Some(OutputEndpoint::IpcEvent(s)) => s,
            _ => return Err(anyhow!("ipc_event_out not wired")),
        };
        let (evt_tx, mut evt_rx) = mpsc::channel::<Event>(64);
        tokio::spawn(async move {
            while let Some(ev) = evt_rx.recv().await { ipc_out.send(ev); }
        });

        let handle = topic::spawn(
            evt_tx,
            PathBuf::from(&self.model_path),
            PathBuf::from(&self.tokenizer_path),
            self.similarity_threshold,
            self.max_block_secs,
            self.silence_block_secs,
        );

        self.inner = Some(handle);
        self.status = NodeStatus::Running;
        let (stx, _) = mpsc::channel(1);
        Ok(NodeHandle::new(stx))
    }

    async fn flush(&mut self, _: FlushSignal) -> FlushAck {
        FlushAck { node: NodeId::Topic, request_id: None }
    }

    async fn shutdown(&mut self) {
        if let Some(h) = self.inner.take() { h.shutdown().await; }
        self.status = NodeStatus::Stopped;
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn topic_ports() -> Vec<PortDescriptor> {
    vec![
        PortDescriptor { name: Cow::Borrowed("ipc_event_out"), port_type: PortType::IpcEvent, direction: Direction::Out },
    ]
}

inventory::submit! {
    NodeFactoryEntry {
        node_type: "topic",
        description: "Topic detection via sentence embeddings",
        factory: |props| Ok(Box::new(TopicNode::new(props)?)),
        ports: topic_ports,
        property_descriptors: || vec![],
    }
}
