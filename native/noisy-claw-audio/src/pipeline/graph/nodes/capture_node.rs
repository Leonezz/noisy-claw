use std::borrow::Cow;
use std::collections::HashMap;

use anyhow::{anyhow, Result};
use tokio::sync::mpsc;

use crate::pipeline::graph::node::{NodeHandle, PipelineNode};
use crate::pipeline::graph::registry::NodeFactoryEntry;
use crate::pipeline::graph::types::{
    Direction, NodeSnapshot, NodeStatus, PortDescriptor, PortType,
    PropType, PropertyDescriptor, PropertyMap,
};
use crate::pipeline::graph::wiring::{InputEndpoint, NodeWiring, OutputEndpoint};
use crate::pipeline::{capture, AudioFrame, FlushAck, FlushSignal, NodeId};

pub struct CaptureNode {
    device: String,
    sample_rate: u32,
    audio_out: Option<OutputEndpoint>,
    inner: Option<capture::Handle>,
    status: NodeStatus,
}

impl CaptureNode {
    pub fn new(props: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            device: props.get("device").and_then(|v| v.as_str())
                .unwrap_or("default").to_string(),
            sample_rate: props.get("sample_rate").and_then(|v| v.as_u64())
                .unwrap_or(48000) as u32,
            audio_out: None,
            inner: None,
            status: NodeStatus::Created,
        })
    }

    /// Access the inner capture handle for operational commands.
    pub fn inner(&self) -> Option<&capture::Handle> {
        self.inner.as_ref()
    }
}

impl NodeWiring for CaptureNode {
    fn accept_input(&mut self, port: &str, _ep: InputEndpoint) -> Result<()> {
        Err(anyhow!("capture: no input port '{port}'"))
    }

    fn set_output(&mut self, port: &str, ep: OutputEndpoint) -> Result<()> {
        match port {
            "audio_out" => { self.audio_out = Some(ep); Ok(()) }
            _ => Err(anyhow!("capture: unknown output '{port}'")),
        }
    }
}

#[async_trait::async_trait]
impl PipelineNode for CaptureNode {
    fn node_type(&self) -> &'static str { "capture" }

    fn ports(&self) -> Vec<PortDescriptor> {
        capture_ports()
    }

    fn property_descriptors(&self) -> Vec<PropertyDescriptor> {
        capture_property_descriptors()
    }

    fn update(&mut self, props: &PropertyMap) -> Result<()> {
        if let Some(v) = props.get("device") {
            if let Some(d) = v.as_str() { self.device = d.to_string(); }
        }
        if let Some(v) = props.get("sample_rate") {
            if let Some(sr) = v.as_u64() { self.sample_rate = sr as u32; }
        }
        Ok(())
    }

    fn snapshot(&self) -> NodeSnapshot {
        let mut p = serde_json::Map::new();
        p.insert("device".into(), serde_json::json!(self.device));
        p.insert("sample_rate".into(), serde_json::json!(self.sample_rate));
        NodeSnapshot {
            node_type: "capture".to_string(),
            status: self.status.clone(),
            properties: p,
            metrics: HashMap::new(),
        }
    }

    async fn start(&mut self) -> Result<NodeHandle> {
        let out = match self.audio_out.take() {
            Some(OutputEndpoint::Audio(s)) => s,
            _ => return Err(anyhow!("audio_out not wired")),
        };

        let (tx, mut rx) = mpsc::unbounded_channel::<AudioFrame>();
        let handle = capture::spawn(tx);
        handle.start(&self.device, self.sample_rate).await;

        tokio::spawn(async move {
            while let Some(f) = rx.recv().await { out.send(f); }
        });

        self.inner = Some(handle);
        self.status = NodeStatus::Running;
        let (stx, _) = mpsc::channel(1);
        Ok(NodeHandle::new(stx))
    }

    async fn flush(&mut self, _: FlushSignal) -> FlushAck {
        FlushAck { node: NodeId::Capture, request_id: None }
    }

    async fn shutdown(&mut self) {
        if let Some(h) = self.inner.take() { h.shutdown().await; }
        self.status = NodeStatus::Stopped;
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn capture_ports() -> Vec<PortDescriptor> {
    vec![PortDescriptor {
        name: Cow::Borrowed("audio_out"),
        port_type: PortType::Audio,
        direction: Direction::Out,
    }]
}

fn capture_property_descriptors() -> Vec<PropertyDescriptor> {
    vec![
        PropertyDescriptor {
            name: Cow::Borrowed("device"),
            value_type: PropType::String,
            default: serde_json::json!("default"),
            description: "Audio capture device name",
        },
        PropertyDescriptor {
            name: Cow::Borrowed("sample_rate"),
            value_type: PropType::Int { min: 8000, max: 96000 },
            default: serde_json::json!(48000),
            description: "Capture sample rate in Hz",
        },
    ]
}

inventory::submit! {
    NodeFactoryEntry {
        node_type: "capture",
        description: "Audio capture from microphone",
        factory: |props| Ok(Box::new(CaptureNode::new(props)?)),
        ports: capture_ports,
        property_descriptors: capture_property_descriptors,
    }
}
