use std::borrow::Cow;
use std::collections::HashMap;

use anyhow::{anyhow, Result};
use tokio::sync::mpsc;

use crate::pipeline::graph::node::{NodeHandle, PipelineNode};
use crate::pipeline::graph::registry::NodeFactoryEntry;
use crate::pipeline::graph::types::{
    Direction, NodeSnapshot, NodeStatus, PortDescriptor, PortType,
    PropertyDescriptor, PropertyMap,
};
use crate::pipeline::graph::wiring::{InputEndpoint, NodeWiring, OutputEndpoint};
use crate::pipeline::{tts, FlushAck, FlushSignal, NodeId, OutputMessage};
use crate::protocol::Event;

pub struct TtsNode {
    output_msg_out: Option<OutputEndpoint>,
    ipc_event_out: Option<OutputEndpoint>,
    inner: Option<tts::Handle>,
    status: NodeStatus,
}

impl TtsNode {
    pub fn new(_props: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            output_msg_out: None,
            ipc_event_out: None,
            inner: None,
            status: NodeStatus::Created,
        })
    }

    pub fn inner(&self) -> Option<&tts::Handle> { self.inner.as_ref() }
}

impl NodeWiring for TtsNode {
    fn accept_input(&mut self, port: &str, _ep: InputEndpoint) -> Result<()> {
        Err(anyhow!("tts: no input port '{port}'"))
    }

    fn set_output(&mut self, port: &str, ep: OutputEndpoint) -> Result<()> {
        match port {
            "output_msg_out" => { self.output_msg_out = Some(ep); Ok(()) }
            "ipc_event_out" => { self.ipc_event_out = Some(ep); Ok(()) }
            _ => Err(anyhow!("tts: unknown output '{port}'")),
        }
    }
}

#[async_trait::async_trait]
impl PipelineNode for TtsNode {
    fn node_type(&self) -> &'static str { "tts" }

    fn ports(&self) -> Vec<PortDescriptor> { tts_ports() }
    fn property_descriptors(&self) -> Vec<PropertyDescriptor> { vec![] }
    fn update(&mut self, _props: &PropertyMap) -> Result<()> { Ok(()) }

    fn snapshot(&self) -> NodeSnapshot {
        NodeSnapshot {
            node_type: "tts".to_string(),
            status: self.status.clone(),
            properties: serde_json::Map::new(),
            metrics: HashMap::new(),
        }
    }

    async fn start(&mut self) -> Result<NodeHandle> {
        // OutputMsg output: bounded → PortSender bridge
        let msg_out = match self.output_msg_out.take() {
            Some(OutputEndpoint::OutputMsg(s)) => s,
            _ => return Err(anyhow!("output_msg_out not wired")),
        };
        let (msg_tx, mut msg_rx) = mpsc::channel::<OutputMessage>(64);
        tokio::spawn(async move {
            while let Some(m) = msg_rx.recv().await { msg_out.send(m); }
        });

        // IpcEvent output: bounded → PortSender bridge
        let ipc_out = match self.ipc_event_out.take() {
            Some(OutputEndpoint::IpcEvent(s)) => s,
            _ => return Err(anyhow!("ipc_event_out not wired")),
        };
        let (evt_tx, mut evt_rx) = mpsc::channel::<Event>(64);
        tokio::spawn(async move {
            while let Some(ev) = evt_rx.recv().await { ipc_out.send(ev); }
        });

        let handle = tts::spawn(msg_tx, evt_tx);
        self.inner = Some(handle);
        self.status = NodeStatus::Running;
        let (stx, _) = mpsc::channel(1);
        Ok(NodeHandle::new(stx))
    }

    async fn flush(&mut self, signal: FlushSignal) -> FlushAck {
        if let Some(ref h) = self.inner {
            return h.flush(signal).await;
        }
        FlushAck { node: NodeId::Tts, request_id: None }
    }

    async fn shutdown(&mut self) {
        if let Some(h) = self.inner.take() { h.shutdown().await; }
        self.status = NodeStatus::Stopped;
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn tts_ports() -> Vec<PortDescriptor> {
    vec![
        PortDescriptor { name: Cow::Borrowed("output_msg_out"), port_type: PortType::OutputMsg, direction: Direction::Out },
        PortDescriptor { name: Cow::Borrowed("ipc_event_out"), port_type: PortType::IpcEvent, direction: Direction::Out },
    ]
}

inventory::submit! {
    NodeFactoryEntry {
        node_type: "tts",
        description: "Text-to-speech synthesis",
        factory: |props| Ok(Box::new(TtsNode::new(props)?)),
        ports: tts_ports,
        property_descriptors: || vec![],
    }
}
