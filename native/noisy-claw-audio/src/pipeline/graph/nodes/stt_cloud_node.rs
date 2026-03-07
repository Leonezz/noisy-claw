use std::borrow::Cow;
use std::collections::HashMap;

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
use crate::pipeline::{stt, AudioFrame, FlushAck, FlushSignal, NodeId};
use crate::protocol::{Event, SttConfig};

pub struct SttCloudNode {
    provider: String,
    api_key: String,
    endpoint: String,
    model: String,
    language: String,
    audio_in: Option<mpsc::UnboundedReceiver<AudioFrame>>,
    vad_in: Option<watch::Receiver<bool>>,
    ipc_event_out: Option<OutputEndpoint>,
    inner: Option<stt::Handle>,
    status: NodeStatus,
    last_error: Option<String>,
}

impl SttCloudNode {
    pub fn new(props: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            provider: props.get("provider").and_then(|v| v.as_str())
                .unwrap_or("").to_string(),
            api_key: props.get("api_key").and_then(|v| v.as_str())
                .unwrap_or("").to_string(),
            endpoint: props.get("endpoint").and_then(|v| v.as_str())
                .unwrap_or("").to_string(),
            model: props.get("model").and_then(|v| v.as_str())
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

    fn build_stt_config(&self) -> SttConfig {
        SttConfig {
            provider: self.provider.clone(),
            api_key: if self.api_key.is_empty() { None } else { Some(self.api_key.clone()) },
            endpoint: if self.endpoint.is_empty() { None } else { Some(self.endpoint.clone()) },
            model: if self.model.is_empty() { None } else { Some(self.model.clone()) },
            languages: Some(vec![self.language.clone()]),
            extra: None,
        }
    }
}

impl NodeWiring for SttCloudNode {
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
            _ => Err(anyhow!("stt_cloud: unknown input '{port}'")),
        }
    }

    fn set_output(&mut self, port: &str, ep: OutputEndpoint) -> Result<()> {
        match port {
            "ipc_event_out" => { self.ipc_event_out = Some(ep); Ok(()) }
            _ => Err(anyhow!("stt_cloud: unknown output '{port}'")),
        }
    }
}

#[async_trait::async_trait]
impl PipelineNode for SttCloudNode {
    fn node_type(&self) -> &'static str { "stt_cloud" }

    fn data_streams(&self) -> Vec<DataStreamDescriptor> {
        vec![DataStreamDescriptor::Metadata {
            name: "stt_cloud".into(),
            fields: vec![
                FieldDescriptor { name: "text".into(), field_type: "string".into() },
                FieldDescriptor { name: "is_final".into(), field_type: "bool".into() },
                FieldDescriptor { name: "confidence".into(), field_type: "f64".into() },
            ],
            node: None,
        }]
    }

    fn ports(&self) -> Vec<PortDescriptor> { stt_cloud_ports() }

    fn property_descriptors(&self) -> Vec<PropertyDescriptor> {
        stt_cloud_property_descriptors()
    }

    fn update(&mut self, props: &PropertyMap) -> Result<()> {
        let mut changed = false;
        if let Some(v) = props.get("provider") {
            if let Some(p) = v.as_str() {
                if p != self.provider { self.provider = p.to_string(); changed = true; }
            }
        }
        if let Some(v) = props.get("api_key") {
            if let Some(k) = v.as_str() {
                if k != self.api_key { self.api_key = k.to_string(); changed = true; }
            }
        }
        if let Some(v) = props.get("endpoint") {
            if let Some(e) = v.as_str() { self.endpoint = e.to_string(); changed = true; }
        }
        if let Some(v) = props.get("model") {
            if let Some(m) = v.as_str() { self.model = m.to_string(); changed = true; }
        }
        if let Some(v) = props.get("language") {
            if let Some(l) = v.as_str() { self.language = l.to_string(); changed = true; }
        }
        // Restart cloud connection if config changed while running
        if changed && !self.provider.is_empty() && !self.api_key.is_empty() {
            if let Some(ref h) = self.inner {
                let _ = h.control_tx.try_send(stt::Control::Stop);
                let _ = h.control_tx.try_send(stt::Control::StartCloud(self.build_stt_config()));
            }
        }
        Ok(())
    }

    async fn command(&mut self, cmd: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        match cmd {
            "start" => {
                let h = self.inner.as_ref().ok_or_else(|| anyhow!("stt_cloud: not started"))?;
                // Accept overrides from args, or use stored properties
                if let Some(provider) = args.get("provider").and_then(|v| v.as_str()) {
                    self.provider = provider.to_string();
                }
                if let Some(api_key) = args.get("api_key").and_then(|v| v.as_str()) {
                    self.api_key = api_key.to_string();
                }
                if let Some(endpoint) = args.get("endpoint").and_then(|v| v.as_str()) {
                    self.endpoint = endpoint.to_string();
                }
                if let Some(model) = args.get("model").and_then(|v| v.as_str()) {
                    self.model = model.to_string();
                }
                if self.provider.is_empty() || self.api_key.is_empty() {
                    return Err(anyhow!("provider and api_key required"));
                }
                h.start_cloud(self.build_stt_config()).await;
                Ok(serde_json::json!({}))
            }
            // Accept legacy start_cloud command for backward compatibility
            "start_cloud" => {
                let h = self.inner.as_ref().ok_or_else(|| anyhow!("stt_cloud: not started"))?;
                let config: SttConfig = serde_json::from_value(args)?;
                self.provider = config.provider.clone();
                self.api_key = config.api_key.clone().unwrap_or_default();
                self.endpoint = config.endpoint.clone().unwrap_or_default();
                self.model = config.model.clone().unwrap_or_default();
                h.start_cloud(config).await;
                Ok(serde_json::json!({}))
            }
            "stop" => {
                let h = self.inner.as_ref().ok_or_else(|| anyhow!("stt_cloud: not started"))?;
                h.stop().await;
                Ok(serde_json::json!({}))
            }
            _ => Err(anyhow!("stt_cloud: unknown command: {cmd}")),
        }
    }

    fn snapshot(&self) -> NodeSnapshot {
        let mut p = serde_json::Map::new();
        p.insert("provider".into(), serde_json::json!(self.provider));
        // Mask api_key in snapshot (only show if set)
        p.insert("api_key".into(), serde_json::json!(
            if self.api_key.is_empty() { "" } else { "••••••••" }
        ));
        p.insert("endpoint".into(), serde_json::json!(self.endpoint));
        p.insert("model".into(), serde_json::json!(self.model));
        p.insert("language".into(), serde_json::json!(self.language));
        NodeSnapshot {
            node_type: "stt_cloud".to_string(),
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

        let handle = stt::spawn(audio_rx, user_speaking_rx, evt_tx, "stt_cloud".into());

        // Auto-start cloud connection if fully configured
        if !self.provider.is_empty() && !self.api_key.is_empty() {
            handle.start_cloud(self.build_stt_config()).await;
            tracing::info!(provider = %self.provider, "stt_cloud: auto-started");
        } else {
            tracing::info!("stt_cloud: started (waiting for provider/api_key config)");
        }

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

fn stt_cloud_ports() -> Vec<PortDescriptor> {
    vec![
        PortDescriptor { name: Cow::Borrowed("audio_in"), port_type: PortType::Audio, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("vad_in"), port_type: PortType::State, direction: Direction::In },
        PortDescriptor { name: Cow::Borrowed("ipc_event_out"), port_type: PortType::IpcEvent, direction: Direction::Out },
    ]
}

fn stt_cloud_property_descriptors() -> Vec<PropertyDescriptor> {
    vec![
        PropertyDescriptor {
            name: Cow::Borrowed("provider"),
            value_type: PropType::String,
            default: serde_json::json!(""),
            description: "Cloud STT provider name",
        },
        PropertyDescriptor {
            name: Cow::Borrowed("api_key"),
            value_type: PropType::String,
            default: serde_json::json!(""),
            description: "API key for the cloud provider",
        },
        PropertyDescriptor {
            name: Cow::Borrowed("endpoint"),
            value_type: PropType::String,
            default: serde_json::json!(""),
            description: "Custom endpoint URL (optional)",
        },
        PropertyDescriptor {
            name: Cow::Borrowed("model"),
            value_type: PropType::String,
            default: serde_json::json!(""),
            description: "Model name (provider-specific)",
        },
        PropertyDescriptor {
            name: Cow::Borrowed("language"),
            value_type: PropType::String,
            default: serde_json::json!("en"),
            description: "Language code",
        },
    ]
}

inventory::submit! {
    NodeFactoryEntry {
        node_type: "stt_cloud",
        description: "Cloud speech-to-text (streaming)",
        factory: |props| Ok(Box::new(SttCloudNode::new(props)?)),
        ports: stt_cloud_ports,
        property_descriptors: stt_cloud_property_descriptors,
    }
}
