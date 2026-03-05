use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Write;

use anyhow::{anyhow, Result};
use tokio::sync::mpsc;

use crate::pipeline::graph::node::{NodeHandle, PipelineNode};
use crate::pipeline::graph::registry::NodeFactoryEntry;
use crate::pipeline::graph::types::{
    Direction, NodeSnapshot, NodeStatus, PortDescriptor, PortType,
    PropertyDescriptor, PropertyMap,
};
use crate::pipeline::graph::wiring::{InputEndpoint, NodeWiring, OutputEndpoint};
use crate::pipeline::{FlushAck, FlushSignal, NodeId};
use crate::protocol::Event;

/// Sink node that collects IPC events from the graph and writes them to stdout.
/// Also provides an event sender for the orchestrator and a transcript tap.
pub struct IpcSinkNode {
    event_in: Option<mpsc::UnboundedReceiver<Event>>,
    /// Sender the orchestrator can use to inject events (Ready, Error, SpeakDone, etc.)
    orchestrator_event_tx: Option<mpsc::Sender<Event>>,
    /// Receiver for final transcript text, used by the orchestrator for topic detection.
    transcript_tap_rx: Option<mpsc::Receiver<String>>,
    status: NodeStatus,
}

impl IpcSinkNode {
    pub fn new(_props: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            event_in: None,
            orchestrator_event_tx: None,
            transcript_tap_rx: None,
            status: NodeStatus::Created,
        })
    }

    /// Take the event sender for the orchestrator. Call after start().
    pub fn take_event_tx(&mut self) -> Option<mpsc::Sender<Event>> {
        self.orchestrator_event_tx.take()
    }

    /// Take the transcript tap receiver. Call after start().
    pub fn take_transcript_tap_rx(&mut self) -> Option<mpsc::Receiver<String>> {
        self.transcript_tap_rx.take()
    }
}

impl NodeWiring for IpcSinkNode {
    fn accept_input(&mut self, port: &str, ep: InputEndpoint) -> Result<()> {
        match port {
            "event_in" => match ep {
                InputEndpoint::IpcEvent(rx) => { self.event_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("event_in expects IpcEvent")),
            },
            _ => Err(anyhow!("ipc_sink: unknown input '{port}'")),
        }
    }

    fn set_output(&mut self, port: &str, _ep: OutputEndpoint) -> Result<()> {
        Err(anyhow!("ipc_sink: no output port '{port}'"))
    }
}

#[async_trait::async_trait]
impl PipelineNode for IpcSinkNode {
    fn node_type(&self) -> &'static str { "ipc_sink" }

    fn ports(&self) -> Vec<PortDescriptor> { ipc_sink_ports() }
    fn property_descriptors(&self) -> Vec<PropertyDescriptor> { vec![] }
    fn update(&mut self, _props: &PropertyMap) -> Result<()> { Ok(()) }

    fn snapshot(&self) -> NodeSnapshot {
        NodeSnapshot {
            node_type: "ipc_sink".to_string(),
            status: self.status.clone(),
            properties: serde_json::Map::new(),
            metrics: HashMap::new(),
        }
    }

    async fn start(&mut self) -> Result<NodeHandle> {
        let mut graph_rx = self.event_in.take()
            .ok_or_else(|| anyhow!("event_in not wired"))?;

        // Merge channel: graph events + orchestrator events → unified stream
        let (merge_tx, mut merge_rx) = mpsc::channel::<Event>(256);
        let orchestrator_tx = merge_tx.clone();
        self.orchestrator_event_tx = Some(orchestrator_tx);

        // Bridge graph unbounded → merge channel
        tokio::spawn(async move {
            while let Some(ev) = graph_rx.recv().await {
                if merge_tx.send(ev).await.is_err() { break; }
            }
        });

        // Transcript tap
        let (tap_tx, tap_rx) = mpsc::channel::<String>(64);
        self.transcript_tap_rx = Some(tap_rx);

        // Stdout writer + transcript tap
        tokio::spawn(async move {
            while let Some(event) = merge_rx.recv().await {
                if let Event::Transcript { ref text, is_final: true, .. } = event {
                    let _ = tap_tx.try_send(text.clone());
                }
                if let Ok(json) = serde_json::to_string(&event) {
                    let stdout = std::io::stdout();
                    let mut stdout = stdout.lock();
                    let _ = writeln!(stdout, "{}", json);
                    let _ = stdout.flush();
                }
            }
        });

        self.status = NodeStatus::Running;
        let (stx, _) = mpsc::channel(1);
        Ok(NodeHandle::new(stx))
    }

    async fn flush(&mut self, _: FlushSignal) -> FlushAck {
        FlushAck { node: NodeId::Capture, request_id: None }
    }

    async fn shutdown(&mut self) {
        self.status = NodeStatus::Stopped;
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn ipc_sink_ports() -> Vec<PortDescriptor> {
    vec![
        PortDescriptor { name: Cow::Borrowed("event_in"), port_type: PortType::IpcEvent, direction: Direction::In },
    ]
}

inventory::submit! {
    NodeFactoryEntry {
        node_type: "ipc_sink",
        description: "Writes IPC events to stdout",
        factory: |props| Ok(Box::new(IpcSinkNode::new(props)?)),
        ports: ipc_sink_ports,
        property_descriptors: || vec![],
    }
}
