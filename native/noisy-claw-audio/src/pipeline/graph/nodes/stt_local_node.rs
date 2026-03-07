use std::borrow::Cow;
use std::collections::HashMap;

use anyhow::{anyhow, Result};
use tokio::sync::{mpsc, watch};

use crate::expand_tilde;
use crate::pipeline::graph::definition::{DataStreamDescriptor, FieldDescriptor};
use crate::pipeline::graph::node::{NodeHandle, PipelineNode};
use crate::pipeline::graph::registry::NodeFactoryEntry;
use crate::pipeline::graph::types::{
    Direction, NodeSnapshot, NodeStatus, PortDescriptor, PortType,
    PropType, PropertyDescriptor, PropertyMap,
};
use crate::pipeline::graph::wiring::{InputEndpoint, NodeWiring, OutputEndpoint};
use crate::pipeline::{stt, AudioFrame, FlushAck, FlushSignal, NodeId};
use crate::protocol::Event;

pub struct SttLocalNode {
    model_path: String,
    language: String,
    audio_in: Option<mpsc::UnboundedReceiver<AudioFrame>>,
    vad_in: Option<watch::Receiver<bool>>,
    ipc_event_out: Option<OutputEndpoint>,
    inner: Option<stt::Handle>,
    status: NodeStatus,
    last_error: Option<String>,
}

impl SttLocalNode {
    pub fn new(props: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            model_path: props.get("model_path").and_then(|v| v.as_str())
                .unwrap_or("").to_string(),
            language: props.get("language").and_then(|v| v.as_str())
                .unwrap_or("en").to_string(),
            audio_in: None,
            vad_in: None,
            ipc_event_out: None,
            inner: None,
            status: NodeStatus::Created,
            last_error: None,
        })
    }
}

impl NodeWiring for SttLocalNode {
    fn accept_input(&mut self, port: &str, ep: InputEndpoint) -> Result<()> {
        match port {
            "audio_in" => match ep {
                InputEndpoint::Audio(rx) => { self.audio_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("audio_in expects Audio")),
            },
            "vad_in" => match ep {
                InputEndpoint::State(rx) => { self.vad_in = Some(rx); Ok(()) }
                _ => Err(anyhow!("vad_in expects State")),
            },
            _ => Err(anyhow!("stt_local: unknown input '{port}'")),
        }
    }

    fn set_output(&mut self, port: &str, ep: OutputEndpoint) -> Result<()> {
        match port {
            "ipc_event_out" => { self.ipc_event_out = Some(ep); Ok(()) }
            _ => Err(anyhow!("stt_local: unknown output '{port}'")),
        }
    }
}

#[async_trait::async_trait]
impl PipelineNode for SttLocalNode {
    fn node_type(&self) -> &'static str { "stt_local" }

    fn data_streams(&self) -> Vec<DataStreamDescriptor> {
        vec![DataStreamDescriptor::Metadata {
            name: "stt_local".into(),
            fields: vec![
                FieldDescriptor { name: "text".into(), field_type: "string".into() },
                FieldDescriptor { name: "is_final".into(), field_type: "bool".into() },
                FieldDescriptor { name: "confidence".into(), field_type: "f64".into() },
            ],
            node: None,
        }]
    }

    fn ports(&self) -> Vec<PortDescriptor> { stt_local_ports() }

    fn property_descriptors(&self) -> Vec<PropertyDescriptor> {
        vec![
            PropertyDescriptor {
                name: Cow::Borrowed("model_path"),
                value_type: PropType::String,
                default: serde_json::json!(""),
                description: "Path to local Whisper GGML model",
            },
            PropertyDescriptor {
                name: Cow::Borrowed("language"),
                value_type: PropType::String,
                default: serde_json::json!("en"),
                description: "Language code for transcription",
            },
        ]
    }

    fn update(&mut self, props: &PropertyMap) -> Result<()> {
        let mut changed = false;
        if let Some(v) = props.get("model_path") {
            if let Some(p) = v.as_str() {
                if p != self.model_path { self.model_path = p.to_string(); changed = true; }
            }
        }
        if let Some(v) = props.get("language") {
            if let Some(l) = v.as_str() {
                if l != self.language { self.language = l.to_string(); changed = true; }
            }
        }
        // Restart STT engine if config changed while running
        if changed {
            if let Some(ref h) = self.inner {
                let _ = h.control_tx.try_send(stt::Control::Stop);
                if !self.model_path.is_empty() {
                    let _ = h.control_tx.try_send(stt::Control::StartLocal {
                        model_path: expand_tilde(&self.model_path),
                        language: self.language.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    async fn command(&mut self, cmd: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        match cmd {
            "start" => {
                let h = self.inner.as_ref().ok_or_else(|| anyhow!("stt_local: not started"))?;
                let model_path = args.get("model_path").and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.model_path.clone());
                let language = args.get("language").and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.language.clone());
                if model_path.is_empty() {
                    return Err(anyhow!("model_path required"));
                }
                self.model_path = model_path.clone();
                self.language = language.clone();
                h.start_local(expand_tilde(&model_path), language).await;
                Ok(serde_json::json!({}))
            }
            "stop" => {
                let h = self.inner.as_ref().ok_or_else(|| anyhow!("stt_local: not started"))?;
                h.stop().await;
                Ok(serde_json::json!({}))
            }
            _ => Err(anyhow!("stt_local: unknown command: {cmd}")),
        }
    }

    fn snapshot(&self) -> NodeSnapshot {
        let mut p = serde_json::Map::new();
        p.insert("model_path".into(), serde_json::json!(self.model_path));
        p.insert("language".into(), serde_json::json!(self.language));
        NodeSnapshot {
            node_type: "stt_local".to_string(),
            status: self.status.clone(),
            properties: p,
            metrics: HashMap::new(),
            last_error: self.last_error.clone(),
        }
    }

    async fn start(&mut self) -> Result<NodeHandle> {
        let audio_rx = self.audio_in.take()
            .ok_or_else(|| anyhow!("audio_in not wired"))?;
        let user_speaking_rx = self.vad_in.take()
            .ok_or_else(|| anyhow!("vad_in not wired"))?;

        let (evt_tx, mut evt_rx) = mpsc::channel::<Event>(64);
        if let Some(OutputEndpoint::IpcEvent(ipc_out)) = self.ipc_event_out.take() {
            tokio::spawn(async move {
                while let Some(ev) = evt_rx.recv().await { ipc_out.send(ev); }
            });
        } else {
            tokio::spawn(async move { while evt_rx.recv().await.is_some() {} });
        }

        let handle = stt::spawn(audio_rx, user_speaking_rx, evt_tx, "stt_local".into());

        // Auto-start Whisper if model_path is configured
        if !self.model_path.is_empty() {
            handle.start_local(expand_tilde(&self.model_path), self.language.clone()).await;
            self.status = NodeStatus::Running;
            self.last_error = None;
            tracing::info!(model = %self.model_path, lang = %self.language, "stt_local: auto-started");
        } else {
            self.status = NodeStatus::Running;
            tracing::info!("stt_local: started (no model configured, waiting for config)");
        }

        self.inner = Some(handle);
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

fn stt_local_ports() -> Vec<PortDescriptor> {
    vec![
        PortDescriptor { name: Cow::Borrowed("audio_in"), port_type: PortType::Audio, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("vad_in"), port_type: PortType::State, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("ipc_event_out"), port_type: PortType::IpcEvent, direction: Direction::Out },
    ]
}

inventory::submit! {
    NodeFactoryEntry {
        node_type: "stt_local",
        description: "Local Whisper speech-to-text",
        factory: |props| Ok(Box::new(SttLocalNode::new(props)?)),
        ports: stt_local_ports,
        property_descriptors: || vec![
            PropertyDescriptor {
                name: Cow::Borrowed("model_path"),
                value_type: PropType::String,
                default: serde_json::json!(""),
                description: "Path to local Whisper GGML model",
            },
            PropertyDescriptor {
                name: Cow::Borrowed("language"),
                value_type: PropType::String,
                default: serde_json::json!("en"),
                description: "Language code for transcription",
            },
        ],
    }
}
