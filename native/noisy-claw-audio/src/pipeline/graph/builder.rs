use std::collections::HashMap;

use anyhow::{anyhow, Result};
use tokio::sync::mpsc;

use crate::pipeline::{AudioFrame, OutputMessage, VadEvent};
use crate::protocol::Event;

use super::definition::{parse_port_ref, PipelineDefinition};
use super::node::PipelineNode;
use super::registry::NodeRegistry;
use super::types::{PortSender, PortType};
use super::wiring::{InputEndpoint, OutputEndpoint};

/// Key for identifying a specific port on a specific node.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PortKey {
    node: String,
    port: String,
}

impl PortKey {
    fn new(node: &str, port: &str) -> Self {
        Self {
            node: node.to_string(),
            port: port.to_string(),
        }
    }
}

pub struct PipelineBuilder {
    nodes: HashMap<String, Box<dyn PipelineNode>>,
    definition: PipelineDefinition,
}

impl PipelineBuilder {
    /// Create a builder from a pipeline definition.
    /// Creates all nodes via the registry but does NOT wire or start them.
    pub fn new(def: &PipelineDefinition) -> Result<Self> {
        let mut nodes = HashMap::new();
        for node_def in &def.nodes {
            let node = NodeRegistry::create(&node_def.node_type, &node_def.properties)?;
            nodes.insert(node_def.name.clone(), node);
        }
        Ok(Self {
            nodes,
            definition: def.clone(),
        })
    }

    /// Validate that two port types are compatible for linking.
    pub fn validate_port_types(src: PortType, dst: PortType) -> Result<()> {
        if src != dst {
            return Err(anyhow!(
                "incompatible port types: {src:?} → {dst:?}"
            ));
        }
        Ok(())
    }

    /// Wire all links and return the built pipeline nodes.
    pub fn build(mut self) -> Result<BuiltPipeline> {
        // Collect port type info for validation
        let mut port_types: HashMap<PortKey, PortType> = HashMap::new();
        for (name, node) in &self.nodes {
            for pd in node.ports() {
                port_types.insert(
                    PortKey::new(name, &pd.name),
                    pd.port_type,
                );
            }
        }

        // Track channels for fan-in (reuse receiver) and fan-out (accumulate senders)
        let mut input_channels: HashMap<PortKey, InputChannelState> = HashMap::new();
        let mut output_senders: HashMap<PortKey, Vec<OutputSenderEntry>> = HashMap::new();

        for link in &self.definition.links {
            let (src_node, src_port) = parse_port_ref(&link.from)?;
            let (dst_node, dst_port) = parse_port_ref(&link.to)?;

            let src_key = PortKey::new(src_node, src_port);
            let dst_key = PortKey::new(dst_node, dst_port);

            let src_type = port_types.get(&src_key)
                .ok_or_else(|| anyhow!("unknown port: {}", link.from))?;
            let dst_type = port_types.get(&dst_key)
                .ok_or_else(|| anyhow!("unknown port: {}", link.to))?;

            Self::validate_port_types(*src_type, *dst_type)?;

            // Create or reuse channel based on port type
            let sender = match src_type {
                PortType::Audio => {
                    let entry = input_channels
                        .entry(dst_key)
                        .or_insert_with(|| InputChannelState::new_audio());
                    entry.clone_audio_tx()
                }
                PortType::VadEvent => {
                    let entry = input_channels
                        .entry(dst_key)
                        .or_insert_with(|| InputChannelState::new_vad_event());
                    entry.clone_vad_event_tx()
                }
                PortType::OutputMsg => {
                    let entry = input_channels
                        .entry(dst_key)
                        .or_insert_with(|| InputChannelState::new_output_msg());
                    entry.clone_output_msg_tx()
                }
                PortType::IpcEvent => {
                    let entry = input_channels
                        .entry(dst_key)
                        .or_insert_with(|| InputChannelState::new_ipc_event());
                    entry.clone_ipc_event_tx()
                }
                PortType::Signal => {
                    let entry = input_channels
                        .entry(dst_key)
                        .or_insert_with(|| InputChannelState::new_signal());
                    entry.clone_signal_tx()
                }
            };

            output_senders
                .entry(src_key)
                .or_default()
                .push(sender);
        }

        // Inject input receivers into nodes
        for (key, state) in input_channels {
            let node = self.nodes.get_mut(&key.node)
                .ok_or_else(|| anyhow!("node not found: {}", key.node))?;
            let endpoint = state.take_input_endpoint()?;
            node.accept_input(&key.port, endpoint)?;
        }

        // Inject output senders into nodes (Direct or FanOut)
        for (key, senders) in output_senders {
            let node = self.nodes.get_mut(&key.node)
                .ok_or_else(|| anyhow!("node not found: {}", key.node))?;
            let endpoint = build_output_endpoint(senders);
            node.set_output(&key.port, endpoint)?;
        }

        Ok(BuiltPipeline {
            nodes: self.nodes,
            definition: self.definition,
        })
    }
}

pub struct BuiltPipeline {
    pub nodes: HashMap<String, Box<dyn PipelineNode>>,
    pub definition: PipelineDefinition,
}

// ── Internal channel state helpers ──────────────────────────

enum InputChannelState {
    Audio {
        tx: mpsc::UnboundedSender<AudioFrame>,
        rx: Option<mpsc::UnboundedReceiver<AudioFrame>>,
    },
    VadEvent {
        tx: mpsc::UnboundedSender<VadEvent>,
        rx: Option<mpsc::UnboundedReceiver<VadEvent>>,
    },
    OutputMsg {
        tx: mpsc::UnboundedSender<OutputMessage>,
        rx: Option<mpsc::UnboundedReceiver<OutputMessage>>,
    },
    IpcEvent {
        tx: mpsc::UnboundedSender<Event>,
        rx: Option<mpsc::UnboundedReceiver<Event>>,
    },
    Signal {
        tx: mpsc::UnboundedSender<()>,
        rx: Option<mpsc::UnboundedReceiver<()>>,
    },
}

impl InputChannelState {
    fn new_audio() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self::Audio { tx, rx: Some(rx) }
    }
    fn new_vad_event() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self::VadEvent { tx, rx: Some(rx) }
    }
    fn new_output_msg() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self::OutputMsg { tx, rx: Some(rx) }
    }
    fn new_ipc_event() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self::IpcEvent { tx, rx: Some(rx) }
    }
    fn new_signal() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self::Signal { tx, rx: Some(rx) }
    }

    fn clone_audio_tx(&self) -> OutputSenderEntry {
        match self {
            Self::Audio { tx, .. } => OutputSenderEntry::Audio(tx.clone()),
            _ => panic!("type mismatch"),
        }
    }
    fn clone_vad_event_tx(&self) -> OutputSenderEntry {
        match self {
            Self::VadEvent { tx, .. } => OutputSenderEntry::VadEvent(tx.clone()),
            _ => panic!("type mismatch"),
        }
    }
    fn clone_output_msg_tx(&self) -> OutputSenderEntry {
        match self {
            Self::OutputMsg { tx, .. } => OutputSenderEntry::OutputMsg(tx.clone()),
            _ => panic!("type mismatch"),
        }
    }
    fn clone_ipc_event_tx(&self) -> OutputSenderEntry {
        match self {
            Self::IpcEvent { tx, .. } => OutputSenderEntry::IpcEvent(tx.clone()),
            _ => panic!("type mismatch"),
        }
    }
    fn clone_signal_tx(&self) -> OutputSenderEntry {
        match self {
            Self::Signal { tx, .. } => OutputSenderEntry::Signal(tx.clone()),
            _ => panic!("type mismatch"),
        }
    }

    fn take_input_endpoint(self) -> Result<InputEndpoint> {
        match self {
            Self::Audio { rx, .. } => Ok(InputEndpoint::Audio(
                rx.ok_or_else(|| anyhow!("audio rx already taken"))?
            )),
            Self::VadEvent { rx, .. } => Ok(InputEndpoint::VadEvent(
                rx.ok_or_else(|| anyhow!("vad_event rx already taken"))?
            )),
            Self::OutputMsg { rx, .. } => Ok(InputEndpoint::OutputMsg(
                rx.ok_or_else(|| anyhow!("output_msg rx already taken"))?
            )),
            Self::IpcEvent { rx, .. } => Ok(InputEndpoint::IpcEvent(
                rx.ok_or_else(|| anyhow!("ipc_event rx already taken"))?
            )),
            Self::Signal { rx, .. } => Ok(InputEndpoint::Signal(
                rx.ok_or_else(|| anyhow!("signal rx already taken"))?
            )),
        }
    }
}

enum OutputSenderEntry {
    Audio(mpsc::UnboundedSender<AudioFrame>),
    VadEvent(mpsc::UnboundedSender<VadEvent>),
    OutputMsg(mpsc::UnboundedSender<OutputMessage>),
    IpcEvent(mpsc::UnboundedSender<Event>),
    Signal(mpsc::UnboundedSender<()>),
}

fn build_output_endpoint(senders: Vec<OutputSenderEntry>) -> OutputEndpoint {
    // All entries in the vec must be the same variant
    if senders.len() == 1 {
        match senders.into_iter().next().unwrap() {
            OutputSenderEntry::Audio(tx) => OutputEndpoint::Audio(PortSender::Direct(tx)),
            OutputSenderEntry::VadEvent(tx) => OutputEndpoint::VadEvent(PortSender::Direct(tx)),
            OutputSenderEntry::OutputMsg(tx) => OutputEndpoint::OutputMsg(PortSender::Direct(tx)),
            OutputSenderEntry::IpcEvent(tx) => OutputEndpoint::IpcEvent(PortSender::Direct(tx)),
            OutputSenderEntry::Signal(tx) => OutputEndpoint::Signal(PortSender::Direct(tx)),
        }
    } else {
        // Fan-out: collect all senders of the same type
        let first = &senders[0];
        match first {
            OutputSenderEntry::Audio(_) => {
                let txs: Vec<_> = senders.into_iter().map(|s| match s {
                    OutputSenderEntry::Audio(tx) => tx,
                    _ => unreachable!(),
                }).collect();
                OutputEndpoint::Audio(PortSender::FanOut(txs))
            }
            OutputSenderEntry::VadEvent(_) => {
                let txs: Vec<_> = senders.into_iter().map(|s| match s {
                    OutputSenderEntry::VadEvent(tx) => tx,
                    _ => unreachable!(),
                }).collect();
                OutputEndpoint::VadEvent(PortSender::FanOut(txs))
            }
            OutputSenderEntry::OutputMsg(_) => {
                let txs: Vec<_> = senders.into_iter().map(|s| match s {
                    OutputSenderEntry::OutputMsg(tx) => tx,
                    _ => unreachable!(),
                }).collect();
                OutputEndpoint::OutputMsg(PortSender::FanOut(txs))
            }
            OutputSenderEntry::IpcEvent(_) => {
                let txs: Vec<_> = senders.into_iter().map(|s| match s {
                    OutputSenderEntry::IpcEvent(tx) => tx,
                    _ => unreachable!(),
                }).collect();
                OutputEndpoint::IpcEvent(PortSender::FanOut(txs))
            }
            OutputSenderEntry::Signal(_) => {
                let txs: Vec<_> = senders.into_iter().map(|s| match s {
                    OutputSenderEntry::Signal(tx) => tx,
                    _ => unreachable!(),
                }).collect();
                OutputEndpoint::Signal(PortSender::FanOut(txs))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_link_incompatible_types() {
        let result = PipelineBuilder::validate_port_types(PortType::Audio, PortType::VadEvent);
        assert!(result.is_err());
    }

    #[test]
    fn validate_link_compatible_types() {
        let result = PipelineBuilder::validate_port_types(PortType::Audio, PortType::Audio);
        assert!(result.is_ok());
    }

    #[test]
    fn builder_from_empty_definition() {
        let def = PipelineDefinition {
            name: "empty".to_string(),
            nodes: vec![],
            links: vec![],
            modes: HashMap::new(),
            data_streams: vec![],
        };
        let result = PipelineBuilder::new(&def);
        assert!(result.is_ok());
    }

    #[test]
    fn builder_rejects_unknown_node_type() {
        let def = PipelineDefinition {
            name: "bad".to_string(),
            nodes: vec![super::super::definition::NodeDefinition {
                name: "x".to_string(),
                node_type: "nonexistent".to_string(),
                properties: serde_json::json!({}),
                ports: vec![],
            }],
            links: vec![],
            modes: HashMap::new(),
            data_streams: vec![],
        };
        let result = PipelineBuilder::new(&def);
        assert!(result.is_err());
    }
}
