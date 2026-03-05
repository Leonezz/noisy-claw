use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use tokio::sync::{mpsc, watch};

use crate::pipeline::graph::node::{NodeHandle, PipelineNode};
use crate::pipeline::graph::registry::NodeFactoryEntry;
use crate::pipeline::graph::types::{
    Direction, NodeSnapshot, NodeStatus, PortDescriptor, PortType,
    PropType, PropertyDescriptor, PropertyMap,
};
use crate::pipeline::graph::wiring::{InputEndpoint, NodeWiring, OutputEndpoint};
use crate::pipeline::{vad, AudioFrame, FlushAck, FlushSignal, NodeId, VadEvent};
use crate::protocol::Event;

pub struct VadNode {
    model_path: String,
    threshold: f32,
    speaking_tts_tx: watch::Sender<bool>,
    audio_in: Option<mpsc::UnboundedReceiver<AudioFrame>>,
    audio_out: Option<OutputEndpoint>,
    vad_event_out: Option<OutputEndpoint>,
    ipc_event_out: Option<OutputEndpoint>,
    barge_in_out: Option<OutputEndpoint>,
    /// Internal barge-in receiver exposed to the orchestrator (when not graph-wired).
    internal_barge_in_rx: Option<mpsc::Receiver<()>>,
    inner: Option<vad::Handle>,
    status: NodeStatus,
}

impl VadNode {
    pub fn new(props: &serde_json::Value) -> Result<Self> {
        let (speaking_tts_tx, _) = watch::channel(false);
        Ok(Self {
            model_path: props.get("model_path").and_then(|v| v.as_str())
                .unwrap_or("").to_string(),
            threshold: props.get("threshold").and_then(|v| v.as_f64())
                .unwrap_or(0.5) as f32,
            speaking_tts_tx,
            audio_in: None,
            audio_out: None,
            vad_event_out: None,
            ipc_event_out: None,
            barge_in_out: None,
            internal_barge_in_rx: None,
            inner: None,
            status: NodeStatus::Created,
        })
    }

    pub fn inner(&self) -> Option<&vad::Handle> { self.inner.as_ref() }

    /// Access the TTS speaking state sender for external control.
    pub fn speaking_tts_tx(&self) -> &watch::Sender<bool> { &self.speaking_tts_tx }

    /// Take the barge-in receiver. Call after start().
    /// Returns None if barge_in_out was wired through the graph instead.
    pub fn take_barge_in_rx(&mut self) -> Option<mpsc::Receiver<()>> {
        self.internal_barge_in_rx.take()
    }
}

impl NodeWiring for VadNode {
    fn accept_input(&mut self, port: &str, ep: InputEndpoint) -> Result<()> {
        match port {
            "audio_in" => match ep {
                InputEndpoint::Audio(rx) => { self.audio_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("audio_in expects Audio")),
            },
            _ => Err(anyhow!("vad: unknown input '{port}'")),
        }
    }

    fn set_output(&mut self, port: &str, ep: OutputEndpoint) -> Result<()> {
        match port {
            "audio_out" => { self.audio_out = Some(ep); Ok(()) }
            "vad_event_out" => { self.vad_event_out = Some(ep); Ok(()) }
            "ipc_event_out" => { self.ipc_event_out = Some(ep); Ok(()) }
            "barge_in_out" => { self.barge_in_out = Some(ep); Ok(()) }
            _ => Err(anyhow!("vad: unknown output '{port}'")),
        }
    }
}

#[async_trait::async_trait]
impl PipelineNode for VadNode {
    fn node_type(&self) -> &'static str { "vad" }

    fn ports(&self) -> Vec<PortDescriptor> { vad_ports() }

    fn property_descriptors(&self) -> Vec<PropertyDescriptor> {
        vec![
            PropertyDescriptor {
                name: Cow::Borrowed("model_path"),
                value_type: PropType::String,
                default: serde_json::json!(""),
                description: "Path to Silero VAD model",
            },
            PropertyDescriptor {
                name: Cow::Borrowed("threshold"),
                value_type: PropType::Float { min: 0.0, max: 1.0 },
                default: serde_json::json!(0.5),
                description: "Speech detection threshold",
            },
        ]
    }

    fn update(&mut self, props: &PropertyMap) -> Result<()> {
        if let Some(v) = props.get("threshold") {
            if let Some(t) = v.as_f64() {
                self.threshold = t as f32;
                if let Some(ref h) = self.inner {
                    let _ = h.control_tx.try_send(vad::Control::SetThreshold(self.threshold));
                }
            }
        }
        if let Some(v) = props.get("speaking_tts") {
            if let Some(b) = v.as_bool() {
                let _ = self.speaking_tts_tx.send(b);
            }
        }
        Ok(())
    }

    fn snapshot(&self) -> NodeSnapshot {
        let mut p = serde_json::Map::new();
        p.insert("model_path".into(), serde_json::json!(self.model_path));
        p.insert("threshold".into(), serde_json::json!(self.threshold));
        p.insert("speaking_tts".into(), serde_json::json!(*self.speaking_tts_tx.borrow()));
        NodeSnapshot {
            node_type: "vad".to_string(),
            status: self.status.clone(),
            properties: p,
            metrics: HashMap::new(),
        }
    }

    async fn start(&mut self) -> Result<NodeHandle> {
        let audio_rx = self.audio_in.take()
            .ok_or_else(|| anyhow!("audio_in not wired"))?;

        let audio_out = match self.audio_out.take() {
            Some(OutputEndpoint::Audio(s)) => s,
            _ => return Err(anyhow!("audio_out not wired")),
        };
        let vad_event_out = match self.vad_event_out.take() {
            Some(OutputEndpoint::VadEvent(s)) => s,
            _ => return Err(anyhow!("vad_event_out not wired")),
        };
        let ipc_event_out = match self.ipc_event_out.take() {
            Some(OutputEndpoint::IpcEvent(s)) => s,
            _ => return Err(anyhow!("ipc_event_out not wired")),
        };
        // Audio output: unbounded bridge
        let (audio_tx, mut audio_bridge_rx) = mpsc::unbounded_channel::<AudioFrame>();
        tokio::spawn(async move {
            while let Some(f) = audio_bridge_rx.recv().await { audio_out.send(f); }
        });

        // VadEvent output: bounded → PortSender bridge
        let (vad_ev_tx, mut vad_ev_rx) = mpsc::channel::<VadEvent>(64);
        tokio::spawn(async move {
            while let Some(ev) = vad_ev_rx.recv().await { vad_event_out.send(ev); }
        });

        // IpcEvent output: bounded → PortSender bridge
        let (evt_tx, mut evt_rx) = mpsc::channel::<Event>(64);
        tokio::spawn(async move {
            while let Some(ev) = evt_rx.recv().await { ipc_event_out.send(ev); }
        });

        // Signal output: graph-wired bridge OR internal channel for orchestrator
        let sig_tx = if let Some(OutputEndpoint::Signal(barge_in_out)) = self.barge_in_out.take() {
            let (sig_tx, mut sig_rx) = mpsc::channel::<()>(16);
            tokio::spawn(async move {
                while sig_rx.recv().await.is_some() { barge_in_out.send(()); }
            });
            sig_tx
        } else {
            let (sig_tx, sig_rx) = mpsc::channel::<()>(16);
            self.internal_barge_in_rx = Some(sig_rx);
            sig_tx
        };

        let speaking_tts_rx = self.speaking_tts_tx.subscribe();
        let handle = vad::spawn(
            audio_rx,
            audio_tx,
            vad_ev_tx,
            evt_tx,
            sig_tx,
            speaking_tts_rx,
            PathBuf::from(&self.model_path),
            self.threshold,
        );

        self.inner = Some(handle);
        self.status = NodeStatus::Running;
        let (stx, _) = mpsc::channel(1);
        Ok(NodeHandle::new(stx))
    }

    async fn flush(&mut self, _: FlushSignal) -> FlushAck {
        FlushAck { node: NodeId::Vad, request_id: None }
    }

    async fn shutdown(&mut self) {
        if let Some(h) = self.inner.take() { h.shutdown().await; }
        self.status = NodeStatus::Stopped;
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn vad_ports() -> Vec<PortDescriptor> {
    vec![
        PortDescriptor { name: Cow::Borrowed("audio_in"), port_type: PortType::Audio, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("audio_out"), port_type: PortType::Audio, direction: Direction::Out },
        PortDescriptor { name: Cow::Borrowed("vad_event_out"), port_type: PortType::VadEvent, direction: Direction::Out },
        PortDescriptor { name: Cow::Borrowed("ipc_event_out"), port_type: PortType::IpcEvent, direction: Direction::Out },
        PortDescriptor { name: Cow::Borrowed("barge_in_out"), port_type: PortType::Signal, direction: Direction::Out },
    ]
}

inventory::submit! {
    NodeFactoryEntry {
        node_type: "vad",
        description: "Voice activity detection with barge-in support",
        factory: |props| Ok(Box::new(VadNode::new(props)?)),
        ports: vad_ports,
        property_descriptors: || vec![],
    }
}
