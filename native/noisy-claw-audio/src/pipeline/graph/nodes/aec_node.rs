use std::borrow::Cow;
use std::collections::HashMap;

use anyhow::{anyhow, Result};
use tokio::sync::mpsc;

use crate::pipeline::graph::definition::DataStreamDescriptor;
use crate::pipeline::graph::node::{NodeHandle, PipelineNode};
use crate::pipeline::graph::registry::NodeFactoryEntry;
use crate::pipeline::graph::types::{
    Direction, NodeSnapshot, NodeStatus, PortDescriptor, PortType,
    PropertyDescriptor, PropertyMap,
};
use crate::pipeline::graph::wiring::{InputEndpoint, NodeWiring, OutputEndpoint};
use crate::pipeline::{aec, AudioFrame, FlushAck, FlushSignal, NodeId};

pub struct AecNode {
    capture_in: Option<mpsc::UnboundedReceiver<AudioFrame>>,
    render_in: Option<mpsc::UnboundedReceiver<AudioFrame>>,
    audio_out: Option<OutputEndpoint>,
    inner: Option<aec::Handle>,
    status: NodeStatus,
}

impl AecNode {
    pub fn new(_props: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            capture_in: None,
            render_in: None,
            audio_out: None,
            inner: None,
            status: NodeStatus::Created,
        })
    }

    pub fn inner(&self) -> Option<&aec::Handle> { self.inner.as_ref() }
}

impl NodeWiring for AecNode {
    fn accept_input(&mut self, port: &str, ep: InputEndpoint) -> Result<()> {
        match port {
            "capture_in" => match ep {
                InputEndpoint::Audio(rx) => { self.capture_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("capture_in expects Audio")),
            },
            "render_in" => match ep {
                InputEndpoint::Audio(rx) => { self.render_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("render_in expects Audio")),
            },
            _ => Err(anyhow!("aec: unknown input '{port}'")),
        }
    }

    fn set_output(&mut self, port: &str, ep: OutputEndpoint) -> Result<()> {
        match port {
            "audio_out" => { self.audio_out = Some(ep); Ok(()) }
            _ => Err(anyhow!("aec: unknown output '{port}'")),
        }
    }
}

#[async_trait::async_trait]
impl PipelineNode for AecNode {
    fn node_type(&self) -> &'static str { "aec" }

    fn data_streams(&self) -> Vec<DataStreamDescriptor> {
        vec![
            DataStreamDescriptor::Audio { name: "capture".into(), sample_rate: 48000, node: None },
            DataStreamDescriptor::Audio { name: "render".into(), sample_rate: 48000, node: None },
            DataStreamDescriptor::Audio { name: "aec_out".into(), sample_rate: 48000, node: None },
        ]
    }

    fn ports(&self) -> Vec<PortDescriptor> { aec_ports() }
    fn property_descriptors(&self) -> Vec<PropertyDescriptor> { vec![] }
    fn update(&mut self, _props: &PropertyMap) -> Result<()> { Ok(()) }

    fn snapshot(&self) -> NodeSnapshot {
        NodeSnapshot {
            node_type: "aec".to_string(),
            status: self.status.clone(),
            properties: serde_json::Map::new(),
            metrics: HashMap::new(),
            last_error: None,
        }
    }

    async fn start(&mut self) -> Result<NodeHandle> {
        let cap_rx = self.capture_in.take()
            .ok_or_else(|| anyhow!("capture_in not wired"))?;
        let ren_rx = self.render_in.take()
            .ok_or_else(|| anyhow!("render_in not wired"))?;
        let out = match self.audio_out.take() {
            Some(OutputEndpoint::Audio(s)) => s,
            _ => return Err(anyhow!("audio_out not wired")),
        };

        let (tx, mut rx) = mpsc::unbounded_channel::<AudioFrame>();
        let handle = aec::spawn(cap_rx, ren_rx, tx);

        tokio::spawn(async move {
            while let Some(f) = rx.recv().await { out.send(f); }
        });

        self.inner = Some(handle);
        self.status = NodeStatus::Running;
        let (stx, _) = mpsc::channel(1);
        Ok(NodeHandle::new(stx))
    }

    async fn flush(&mut self, _: FlushSignal) -> FlushAck {
        FlushAck { node: NodeId::Aec, request_id: None }
    }

    async fn shutdown(&mut self) {
        if let Some(h) = self.inner.take() { h.shutdown().await; }
        self.status = NodeStatus::Stopped;
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn aec_ports() -> Vec<PortDescriptor> {
    vec![
        PortDescriptor { name: Cow::Borrowed("capture_in"), port_type: PortType::Audio, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("render_in"), port_type: PortType::Audio, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("audio_out"), port_type: PortType::Audio, direction: Direction::Out },
    ]
}

inventory::submit! {
    NodeFactoryEntry {
        node_type: "aec",
        description: "Echo cancellation with noise suppression",
        factory: |props| Ok(Box::new(AecNode::new(props)?)),
        ports: aec_ports,
        property_descriptors: || vec![],
    }
}
