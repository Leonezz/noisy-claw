use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

// ── Port Types ──────────────────────────────────────────────

/// Sealed enum of all data types that can flow between pipeline nodes.
/// Adding a new variant forces exhaustive handling in the builder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    /// AudioFrame — unbounded channel, low-latency
    Audio,
    /// OutputMessage — bounded channel
    OutputMsg,
    /// protocol::Event — bounded channel
    IpcEvent,
    /// bool — watch channel for observable state (e.g., speaker_active, user_speaking)
    State,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    In,
    Out,
}

/// Describes a port on a pipeline node.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortDescriptor {
    pub name: Cow<'static, str>,
    pub port_type: PortType,
    pub direction: Direction,
}

// ── Node Role (inferred) ────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    Source,
    Sink,
    Transform,
}

impl NodeRole {
    pub fn infer(ports: &[PortDescriptor]) -> Self {
        let has_in = ports.iter().any(|p| p.direction == Direction::In);
        let has_out = ports.iter().any(|p| p.direction == Direction::Out);
        match (has_in, has_out) {
            (false, true) => NodeRole::Source,
            (true, false) => NodeRole::Sink,
            _ => NodeRole::Transform,
        }
    }
}

// ── Node Status ─────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum NodeStatus {
    Created,
    Running,
    Paused,
    #[serde(rename = "error")]
    Error { message: String },
    Stopped,
}

impl fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Error { message } => write!(f, "error: {message}"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

// ── Node Snapshot (introspection) ───────────────────────────

#[derive(Clone, Debug, Serialize)]
pub struct NodeSnapshot {
    pub node_type: String,
    pub status: NodeStatus,
    pub properties: serde_json::Map<String, serde_json::Value>,
    pub metrics: HashMap<String, f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

// ── Property Descriptors ────────────────────────────────────

pub type PropertyMap = serde_json::Map<String, serde_json::Value>;

#[derive(Clone, Debug, Serialize)]
pub struct PropertyDescriptor {
    pub name: Cow<'static, str>,
    pub value_type: PropType,
    pub default: serde_json::Value,
    pub description: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum PropType {
    Bool,
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    String,
    Enum { variants: Vec<&'static str> },
}

// ── PortSender (fan-out support) ────────────────────────────

/// Output wrapper that transparently handles 1:1 or 1:N fan-out.
/// Direct variant has zero overhead vs. a raw mpsc::send.
pub enum PortSender<T> {
    Direct(mpsc::UnboundedSender<T>),
    FanOut(Vec<mpsc::UnboundedSender<T>>),
}

impl<T: Clone> PortSender<T> {
    pub fn send(&self, item: T) {
        match self {
            Self::Direct(tx) => {
                let _ = tx.send(item);
            }
            Self::FanOut(txs) => {
                let last = txs.len().saturating_sub(1);
                for (i, tx) in txs.iter().enumerate() {
                    if i < last {
                        let _ = tx.send(item.clone());
                    } else {
                        let _ = tx.send(item);
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_type_equality() {
        assert_eq!(PortType::Audio, PortType::Audio);
        assert_ne!(PortType::Audio, PortType::State);
    }

    #[test]
    fn port_descriptor_with_static_name() {
        let pd = PortDescriptor {
            name: Cow::Borrowed("audio_out"),
            port_type: PortType::Audio,
            direction: Direction::Out,
        };
        assert_eq!(pd.name, "audio_out");
    }

    #[test]
    fn port_descriptor_with_dynamic_name() {
        let name = format!("audio_in_{}", 3);
        let pd = PortDescriptor {
            name: Cow::Owned(name),
            port_type: PortType::Audio,
            direction: Direction::In,
        };
        assert_eq!(pd.name, "audio_in_3");
    }

    #[test]
    fn node_role_inferred_source() {
        let ports = vec![
            PortDescriptor {
                name: Cow::Borrowed("audio_out"),
                port_type: PortType::Audio,
                direction: Direction::Out,
            },
        ];
        assert_eq!(NodeRole::infer(&ports), NodeRole::Source);
    }

    #[test]
    fn node_role_inferred_sink() {
        let ports = vec![
            PortDescriptor {
                name: Cow::Borrowed("audio_in"),
                port_type: PortType::Audio,
                direction: Direction::In,
            },
        ];
        assert_eq!(NodeRole::infer(&ports), NodeRole::Sink);
    }

    #[test]
    fn node_role_inferred_transform() {
        let ports = vec![
            PortDescriptor {
                name: Cow::Borrowed("audio_in"),
                port_type: PortType::Audio,
                direction: Direction::In,
            },
            PortDescriptor {
                name: Cow::Borrowed("audio_out"),
                port_type: PortType::Audio,
                direction: Direction::Out,
            },
        ];
        assert_eq!(NodeRole::infer(&ports), NodeRole::Transform);
    }

    #[test]
    fn node_status_display() {
        assert_eq!(format!("{}", NodeStatus::Running), "running");
        assert_eq!(format!("{}", NodeStatus::Error { message: "fail".into() }), "error: fail");
    }

    #[test]
    fn property_descriptor_default_value() {
        let pd = PropertyDescriptor {
            name: Cow::Borrowed("threshold"),
            value_type: PropType::Float { min: 0.0, max: 1.0 },
            default: serde_json::json!(0.5),
            description: "Speech detection threshold",
        };
        assert_eq!(pd.default, 0.5);
    }

    #[test]
    fn port_sender_direct_send() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let sender = PortSender::Direct(tx);
        sender.send(42);
        assert_eq!(rx.try_recv().unwrap(), 42);
    }

    #[test]
    fn port_sender_fanout_send() {
        let (tx1, mut rx1) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let sender = PortSender::FanOut(vec![tx1, tx2]);
        sender.send(99);
        assert_eq!(rx1.try_recv().unwrap(), 99);
        assert_eq!(rx2.try_recv().unwrap(), 99);
    }
}
