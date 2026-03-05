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
use crate::pipeline::{
    output, AudioFrame, FlushAck, FlushSignal, NodeId, OutputMessage, OutputNodeEvent,
};

pub struct OutputNode {
    output_msg_in: Option<mpsc::UnboundedReceiver<OutputMessage>>,
    render_ref_out: Option<OutputEndpoint>,
    speak_done_rx: Option<mpsc::Receiver<OutputNodeEvent>>,
    inner: Option<output::Handle>,
    status: NodeStatus,
}

impl OutputNode {
    pub fn new(_props: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            output_msg_in: None,
            render_ref_out: None,
            speak_done_rx: None,
            inner: None,
            status: NodeStatus::Created,
        })
    }

    pub fn inner(&self) -> Option<&output::Handle> { self.inner.as_ref() }

    /// Take the internal SpeakDone receiver. Call after start().
    pub fn take_speak_done_rx(&mut self) -> Option<mpsc::Receiver<OutputNodeEvent>> {
        self.speak_done_rx.take()
    }
}

impl NodeWiring for OutputNode {
    fn accept_input(&mut self, port: &str, ep: InputEndpoint) -> Result<()> {
        match port {
            "output_msg_in" => match ep {
                InputEndpoint::OutputMsg(rx) => { self.output_msg_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("output_msg_in expects OutputMsg")),
            },
            _ => Err(anyhow!("output: unknown input '{port}'")),
        }
    }

    fn set_output(&mut self, port: &str, ep: OutputEndpoint) -> Result<()> {
        match port {
            "render_ref_out" => { self.render_ref_out = Some(ep); Ok(()) }
            _ => Err(anyhow!("output: unknown output '{port}'")),
        }
    }
}

#[async_trait::async_trait]
impl PipelineNode for OutputNode {
    fn node_type(&self) -> &'static str { "output" }

    fn ports(&self) -> Vec<PortDescriptor> { output_ports() }
    fn property_descriptors(&self) -> Vec<PropertyDescriptor> { vec![] }
    fn update(&mut self, _props: &PropertyMap) -> Result<()> { Ok(()) }

    fn snapshot(&self) -> NodeSnapshot {
        NodeSnapshot {
            node_type: "output".to_string(),
            status: self.status.clone(),
            properties: serde_json::Map::new(),
            metrics: HashMap::new(),
        }
    }

    async fn start(&mut self) -> Result<NodeHandle> {
        // OutputMsg input: unbounded → bounded bridge
        let mut msg_unbounded = self.output_msg_in.take()
            .ok_or_else(|| anyhow!("output_msg_in not wired"))?;
        let (msg_tx, msg_rx) = mpsc::channel::<OutputMessage>(64);
        tokio::spawn(async move {
            while let Some(m) = msg_unbounded.recv().await {
                if msg_tx.send(m).await.is_err() { break; }
            }
        });

        // Render ref output: unbounded bridge
        let render_out = match self.render_ref_out.take() {
            Some(OutputEndpoint::Audio(s)) => s,
            _ => return Err(anyhow!("render_ref_out not wired")),
        };
        let (ref_tx, mut ref_rx) = mpsc::unbounded_channel::<AudioFrame>();
        tokio::spawn(async move {
            while let Some(f) = ref_rx.recv().await { render_out.send(f); }
        });

        // Internal SpeakDone channel (not a graph port)
        let (internal_tx, internal_rx) = mpsc::channel::<OutputNodeEvent>(16);
        self.speak_done_rx = Some(internal_rx);

        let handle = output::spawn(msg_rx, ref_tx, internal_tx);
        self.inner = Some(handle);
        self.status = NodeStatus::Running;
        let (stx, _) = mpsc::channel(1);
        Ok(NodeHandle::new(stx))
    }

    async fn flush(&mut self, signal: FlushSignal) -> FlushAck {
        if let Some(ref h) = self.inner {
            return h.flush(signal).await;
        }
        FlushAck { node: NodeId::Output, request_id: None }
    }

    async fn shutdown(&mut self) {
        if let Some(h) = self.inner.take() { h.shutdown().await; }
        self.status = NodeStatus::Stopped;
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn output_ports() -> Vec<PortDescriptor> {
    vec![
        PortDescriptor { name: Cow::Borrowed("output_msg_in"), port_type: PortType::OutputMsg, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("render_ref_out"), port_type: PortType::Audio, direction: Direction::Out },
    ]
}

inventory::submit! {
    NodeFactoryEntry {
        node_type: "output",
        description: "Audio output with playback management",
        factory: |props| Ok(Box::new(OutputNode::new(props)?)),
        ports: output_ports,
        property_descriptors: || vec![],
    }
}
