use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use tokio::sync::{mpsc, watch};

use crate::pipeline::graph::definition::{DataStreamDescriptor, FieldDescriptor};
use crate::pipeline::graph::node::{NodeHandle, PipelineNode};
use crate::pipeline::graph::registry::NodeFactoryEntry;
use crate::pipeline::graph::types::{
    Direction, NodeSnapshot, NodeStatus, PortDescriptor, PortType,
    PropType, PropertyDescriptor, PropertyMap,
};
use crate::pipeline::graph::wiring::{InputEndpoint, NodeWiring, OutputEndpoint};
use crate::expand_tilde;
use crate::pipeline::{vad, AudioFrame, FlushAck, FlushSignal, NodeId};
use crate::protocol::Event;

pub struct VadNode {
    model_path: String,
    threshold: f32,
    audio_in: Option<mpsc::UnboundedReceiver<AudioFrame>>,
    speaker_state_in: Option<watch::Receiver<bool>>,
    audio_out: Option<OutputEndpoint>,
    user_speaking_tx: Option<watch::Sender<bool>>,
    ipc_event_out: Option<OutputEndpoint>,
    /// Subscribed during start() for the orchestrator to observe user_speaking state.
    user_speaking_rx: Option<watch::Receiver<bool>>,
    inner: Option<vad::Handle>,
    status: NodeStatus,
    last_error: Option<String>,
}

impl VadNode {
    pub fn new(props: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            model_path: props.get("model_path").and_then(|v| v.as_str())
                .unwrap_or("").to_string(),
            threshold: props.get("threshold").and_then(|v| v.as_f64())
                .unwrap_or(0.5) as f32,
            audio_in: None,
            speaker_state_in: None,
            audio_out: None,
            user_speaking_tx: None,
            ipc_event_out: None,
            user_speaking_rx: None,
            inner: None,
            status: NodeStatus::Created,
            last_error: None,
        })
    }

    pub fn inner(&self) -> Option<&vad::Handle> { self.inner.as_ref() }

    /// Take the user_speaking watch receiver. Call after start().
    pub fn take_user_speaking_rx(&mut self) -> Option<watch::Receiver<bool>> {
        self.user_speaking_rx.take()
    }
}

impl NodeWiring for VadNode {
    fn accept_input(&mut self, port: &str, ep: InputEndpoint) -> Result<()> {
        match port {
            "audio_in" => match ep {
                InputEndpoint::Audio(rx) => { self.audio_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("audio_in expects Audio")),
            },
            "speaker_state_in" => match ep {
                InputEndpoint::State(rx) => { self.speaker_state_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("speaker_state_in expects State")),
            },
            _ => Err(anyhow!("vad: unknown input '{port}'")),
        }
    }

    fn set_output(&mut self, port: &str, ep: OutputEndpoint) -> Result<()> {
        match port {
            "audio_out" => { self.audio_out = Some(ep); Ok(()) }
            "user_speaking_out" => match ep {
                OutputEndpoint::State(tx) => { self.user_speaking_tx = Some(tx); Ok(()) }
                _ => Err(anyhow!("user_speaking_out expects State")),
            },
            "ipc_event_out" => { self.ipc_event_out = Some(ep); Ok(()) }
            _ => Err(anyhow!("vad: unknown output '{port}'")),
        }
    }
}

#[async_trait::async_trait]
impl PipelineNode for VadNode {
    fn node_type(&self) -> &'static str { "vad" }

    fn data_streams(&self) -> Vec<DataStreamDescriptor> {
        vec![
            DataStreamDescriptor::Audio { name: "vad_pass".into(), sample_rate: 48000, node: None },
            DataStreamDescriptor::Metadata {
                name: "vad".into(),
                fields: vec![
                    FieldDescriptor { name: "speech_prob".into(), field_type: "f64".into() },
                    FieldDescriptor { name: "is_speech".into(), field_type: "bool".into() },
                    FieldDescriptor { name: "speaker_active".into(), field_type: "bool".into() },
                    FieldDescriptor { name: "blanking".into(), field_type: "u32".into() },
                    FieldDescriptor { name: "was_speaking".into(), field_type: "bool".into() },
                ],
                node: None,
            },
        ]
    }

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
                description: "Speech detection threshold (base)",
            },
        ]
    }

    async fn command(&mut self, cmd: &str, _args: serde_json::Value) -> Result<serde_json::Value> {
        match cmd {
            "status" => {
                let initialized = self.inner().map_or(false, |h| h.is_initialized());
                Ok(serde_json::json!({ "initialized": initialized }))
            }
            "reset" => {
                if let Some(h) = self.inner() { h.reset().await; }
                Ok(serde_json::json!({}))
            }
            _ => Err(anyhow!("vad: unknown command: {cmd}")),
        }
    }

    fn update(&mut self, props: &PropertyMap) -> Result<()> {
        if let Some(v) = props.get("model_path") {
            if let Some(p) = v.as_str() { self.model_path = p.to_string(); }
        }
        if let Some(v) = props.get("threshold") {
            if let Some(t) = v.as_f64() {
                self.threshold = t as f32;
                if let Some(ref h) = self.inner {
                    let _ = h.control_tx.try_send(vad::Control::SetThreshold(self.threshold));
                }
            }
        }
        Ok(())
    }

    fn snapshot(&self) -> NodeSnapshot {
        let mut p = serde_json::Map::new();
        p.insert("model_path".into(), serde_json::json!(self.model_path));
        p.insert("threshold".into(), serde_json::json!(self.threshold));
        NodeSnapshot {
            node_type: "vad".to_string(),
            status: self.status.clone(),
            properties: p,
            metrics: HashMap::new(),
            last_error: self.last_error.clone(),
        }
    }

    async fn start(&mut self) -> Result<NodeHandle> {
        let audio_rx = self.audio_in.take()
            .ok_or_else(|| anyhow!("audio_in not wired"))?;

        let audio_out = match self.audio_out.take() {
            Some(OutputEndpoint::Audio(s)) => s,
            _ => return Err(anyhow!("audio_out not wired")),
        };
        // Audio output: unbounded bridge
        let (audio_tx, mut audio_bridge_rx) = mpsc::unbounded_channel::<AudioFrame>();
        tokio::spawn(async move {
            while let Some(f) = audio_bridge_rx.recv().await { audio_out.send(f); }
        });

        // IpcEvent output: optional (not needed in standalone mode)
        let (evt_tx, mut evt_rx) = mpsc::channel::<Event>(64);
        if let Some(OutputEndpoint::IpcEvent(ipc_event_out)) = self.ipc_event_out.take() {
            tokio::spawn(async move {
                while let Some(ev) = evt_rx.recv().await { ipc_event_out.send(ev); }
            });
        } else {
            // Drop events when ipc_event_out is not wired
            tokio::spawn(async move { while evt_rx.recv().await.is_some() {} });
        }

        // user_speaking State output
        let user_speaking_tx = self.user_speaking_tx.take()
            .ok_or_else(|| anyhow!("user_speaking_out not wired"))?;
        // Subscribe for orchestrator before passing sender to inner task
        self.user_speaking_rx = Some(user_speaking_tx.subscribe());

        // speaker_state State input (from output node, optional — defaults to false)
        let speaker_active_rx = self.speaker_state_in.take()
            .unwrap_or_else(|| {
                let (_tx, rx) = watch::channel(false);
                rx
            });

        let handle = vad::spawn(
            audio_rx,
            audio_tx,
            user_speaking_tx,
            evt_tx,
            speaker_active_rx,
            expand_tilde(&self.model_path),
            self.threshold,
        );

        if handle.is_initialized() {
            self.status = NodeStatus::Running;
            self.last_error = None;
        } else {
            let msg = format!("VAD model not found: {}", self.model_path);
            self.status = NodeStatus::Error { message: msg.clone() };
            self.last_error = Some(msg);
        }
        self.inner = Some(handle);
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
        PortDescriptor { name: Cow::Borrowed("speaker_state_in"), port_type: PortType::State, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("audio_out"), port_type: PortType::Audio, direction: Direction::Out },
        PortDescriptor { name: Cow::Borrowed("user_speaking_out"), port_type: PortType::State, direction: Direction::Out },
        PortDescriptor { name: Cow::Borrowed("ipc_event_out"), port_type: PortType::IpcEvent, direction: Direction::Out },
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
