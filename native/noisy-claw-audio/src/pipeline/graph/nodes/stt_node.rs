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
use crate::pipeline::{stt, AudioFrame, FlushAck, FlushSignal, NodeId, VadEvent};
use crate::protocol::Event;

pub struct SttNode {
    audio_in: Option<mpsc::UnboundedReceiver<AudioFrame>>,
    vad_in: Option<mpsc::UnboundedReceiver<VadEvent>>,
    ipc_event_out: Option<OutputEndpoint>,
    inner: Option<stt::Handle>,
    status: NodeStatus,
}

impl SttNode {
    pub fn new(_props: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            audio_in: None,
            vad_in: None,
            ipc_event_out: None,
            inner: None,
            status: NodeStatus::Created,
        })
    }

    pub fn inner(&self) -> Option<&stt::Handle> { self.inner.as_ref() }
}

impl NodeWiring for SttNode {
    fn accept_input(&mut self, port: &str, ep: InputEndpoint) -> Result<()> {
        match port {
            "audio_in" => match ep {
                InputEndpoint::Audio(rx) => { self.audio_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("audio_in expects Audio")),
            },
            "vad_in" => match ep {
                InputEndpoint::VadEvent(rx) => { self.vad_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("vad_in expects VadEvent")),
            },
            _ => Err(anyhow!("stt: unknown input '{port}'")),
        }
    }

    fn set_output(&mut self, port: &str, ep: OutputEndpoint) -> Result<()> {
        match port {
            "ipc_event_out" => { self.ipc_event_out = Some(ep); Ok(()) }
            _ => Err(anyhow!("stt: unknown output '{port}'")),
        }
    }
}

#[async_trait::async_trait]
impl PipelineNode for SttNode {
    fn node_type(&self) -> &'static str { "stt" }

    fn ports(&self) -> Vec<PortDescriptor> { stt_ports() }
    fn property_descriptors(&self) -> Vec<PropertyDescriptor> { vec![] }
    fn update(&mut self, _props: &PropertyMap) -> Result<()> { Ok(()) }

    fn snapshot(&self) -> NodeSnapshot {
        NodeSnapshot {
            node_type: "stt".to_string(),
            status: self.status.clone(),
            properties: serde_json::Map::new(),
            metrics: HashMap::new(),
        }
    }

    async fn start(&mut self) -> Result<NodeHandle> {
        let audio_rx = self.audio_in.take()
            .ok_or_else(|| anyhow!("audio_in not wired"))?;

        // VadEvent input: unbounded → bounded bridge
        let mut vad_unbounded = self.vad_in.take()
            .ok_or_else(|| anyhow!("vad_in not wired"))?;
        let (vad_tx, vad_rx) = mpsc::channel::<VadEvent>(64);
        tokio::spawn(async move {
            while let Some(ev) = vad_unbounded.recv().await {
                if vad_tx.send(ev).await.is_err() { break; }
            }
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

        let handle = stt::spawn(audio_rx, vad_rx, evt_tx);
        self.inner = Some(handle);
        self.status = NodeStatus::Running;
        let (stx, _) = mpsc::channel(1);
        Ok(NodeHandle::new(stx))
    }

    async fn flush(&mut self, _: FlushSignal) -> FlushAck {
        FlushAck { node: NodeId::Stt, request_id: None }
    }

    async fn shutdown(&mut self) {
        if let Some(h) = self.inner.take() { h.shutdown().await; }
        self.status = NodeStatus::Stopped;
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn stt_ports() -> Vec<PortDescriptor> {
    vec![
        PortDescriptor { name: Cow::Borrowed("audio_in"), port_type: PortType::Audio, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("vad_in"), port_type: PortType::VadEvent, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("ipc_event_out"), port_type: PortType::IpcEvent, direction: Direction::Out },
    ]
}

inventory::submit! {
    NodeFactoryEntry {
        node_type: "stt",
        description: "Speech-to-text (cloud or local Whisper)",
        factory: |props| Ok(Box::new(SttNode::new(props)?)),
        ports: stt_ports,
        property_descriptors: || vec![],
    }
}
