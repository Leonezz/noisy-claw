use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::{mpsc, watch};

use crate::pipeline::graph::definition::DataStreamDescriptor;
use crate::pipeline::graph::node::{NodeHandle, PipelineNode};
use crate::pipeline::graph::registry::NodeFactoryEntry;
use crate::pipeline::graph::types::{
    Direction, NodeSnapshot, NodeStatus, PortDescriptor, PortType,
    PropertyDescriptor, PropertyMap,
};
use crate::pipeline::graph::wiring::{InputEndpoint, NodeWiring, OutputEndpoint};
use crate::pipeline::{output, AudioFrame, FlushAck, FlushSignal, NodeId, OutputMessage};
use crate::protocol::Event;

pub struct OutputNode {
    output_msg_in: Option<mpsc::UnboundedReceiver<OutputMessage>>,
    user_speaking_in: Option<watch::Receiver<bool>>,
    render_ref_out: Option<OutputEndpoint>,
    ipc_event_out: Option<OutputEndpoint>,
    speaker_active_tx: Option<Arc<watch::Sender<bool>>>,
    speaker_active_rx: Option<watch::Receiver<bool>>,
    inner: Option<output::Handle>,
    status: NodeStatus,
}

impl OutputNode {
    pub fn new(_props: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            output_msg_in: None,
            user_speaking_in: None,
            render_ref_out: None,
            ipc_event_out: None,
            speaker_active_tx: None,
            speaker_active_rx: None,
            inner: None,
            status: NodeStatus::Created,
        })
    }

    pub fn inner(&self) -> Option<&output::Handle> { self.inner.as_ref() }

    /// Take a speaker_active watch receiver for external observation. Call after start().
    pub fn take_speaker_active_rx(&mut self) -> Option<watch::Receiver<bool>> {
        self.speaker_active_rx.take()
    }
}

impl NodeWiring for OutputNode {
    fn accept_input(&mut self, port: &str, ep: InputEndpoint) -> Result<()> {
        match port {
            "output_msg_in" => match ep {
                InputEndpoint::OutputMsg(rx) => { self.output_msg_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("output_msg_in expects OutputMsg")),
            },
            "user_speaking_in" => match ep {
                InputEndpoint::State(rx) => { self.user_speaking_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("user_speaking_in expects State")),
            },
            _ => Err(anyhow!("output: unknown input '{port}'")),
        }
    }

    fn set_output(&mut self, port: &str, ep: OutputEndpoint) -> Result<()> {
        match port {
            "render_ref_out" => { self.render_ref_out = Some(ep); Ok(()) }
            "ipc_event_out" => { self.ipc_event_out = Some(ep); Ok(()) }
            "speaker_active_out" => match ep {
                OutputEndpoint::State(tx) => { self.speaker_active_tx = Some(Arc::new(tx)); Ok(()) }
                _ => Err(anyhow!("speaker_active_out expects State")),
            },
            _ => Err(anyhow!("output: unknown output '{port}'")),
        }
    }
}

#[async_trait::async_trait]
impl PipelineNode for OutputNode {
    fn node_type(&self) -> &'static str { "output" }

    fn data_streams(&self) -> Vec<DataStreamDescriptor> {
        vec![DataStreamDescriptor::Audio {
            name: "tts_out".into(),
            sample_rate: 24000,
            node: None,
        }]
    }

    fn ports(&self) -> Vec<PortDescriptor> { output_ports() }
    fn property_descriptors(&self) -> Vec<PropertyDescriptor> { vec![] }
    fn update(&mut self, _props: &PropertyMap) -> Result<()> { Ok(()) }

    async fn command(&mut self, cmd: &str, _args: serde_json::Value) -> Result<serde_json::Value> {
        match cmd {
            "status" => {
                let speaking = self.speaker_active_tx.as_ref()
                    .map(|tx| *tx.borrow())
                    .unwrap_or(false);
                Ok(serde_json::json!({ "speaking": speaking }))
            }
            _ => Err(anyhow!("output: unknown command: {cmd}")),
        }
    }

    fn snapshot(&self) -> NodeSnapshot {
        let mut p = serde_json::Map::new();
        if let Some(ref tx) = self.speaker_active_tx {
            p.insert("speaker_active".into(), serde_json::json!(*tx.borrow()));
        }
        NodeSnapshot {
            node_type: "output".to_string(),
            status: self.status.clone(),
            properties: p,
            metrics: HashMap::new(),
            last_error: None,
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

        // IPC event output: bounded → PortSender bridge
        let (evt_tx, mut evt_rx) = mpsc::channel::<Event>(64);
        if let Some(OutputEndpoint::IpcEvent(ipc_out)) = self.ipc_event_out.take() {
            tokio::spawn(async move {
                while let Some(ev) = evt_rx.recv().await { ipc_out.send(ev); }
            });
        } else {
            // Drop events when ipc_event_out is not wired
            tokio::spawn(async move { while evt_rx.recv().await.is_some() {} });
        }

        // Subscribe for external observation before passing sender to inner task
        if let Some(ref tx) = self.speaker_active_tx {
            self.speaker_active_rx = Some(tx.subscribe());
        }

        let user_speaking_rx = self.user_speaking_in.take();
        let handle = output::spawn(msg_rx, ref_tx, evt_tx, self.speaker_active_tx.clone(), user_speaking_rx);
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
        PortDescriptor { name: Cow::Borrowed("user_speaking_in"), port_type: PortType::State, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("render_ref_out"), port_type: PortType::Audio, direction: Direction::Out },
        PortDescriptor { name: Cow::Borrowed("ipc_event_out"), port_type: PortType::IpcEvent, direction: Direction::Out },
        PortDescriptor { name: Cow::Borrowed("speaker_active_out"), port_type: PortType::State, direction: Direction::Out },
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
