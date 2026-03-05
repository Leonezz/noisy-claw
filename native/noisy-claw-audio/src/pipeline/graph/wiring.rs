use anyhow::Result;
use tokio::sync::mpsc;

use crate::pipeline::{AudioFrame, OutputMessage, VadEvent};
use crate::protocol::Event;

use super::types::{PortSender, PortType};

/// Typed input endpoints injected by the pipeline builder.
pub enum InputEndpoint {
    Audio(mpsc::UnboundedReceiver<AudioFrame>),
    VadEvent(mpsc::UnboundedReceiver<VadEvent>),
    OutputMsg(mpsc::UnboundedReceiver<OutputMessage>),
    IpcEvent(mpsc::UnboundedReceiver<Event>),
    Signal(mpsc::UnboundedReceiver<()>),
}

impl InputEndpoint {
    pub fn port_type(&self) -> PortType {
        match self {
            Self::Audio(_) => PortType::Audio,
            Self::VadEvent(_) => PortType::VadEvent,
            Self::OutputMsg(_) => PortType::OutputMsg,
            Self::IpcEvent(_) => PortType::IpcEvent,
            Self::Signal(_) => PortType::Signal,
        }
    }
}

/// Typed output endpoints injected by the pipeline builder.
pub enum OutputEndpoint {
    Audio(PortSender<AudioFrame>),
    VadEvent(PortSender<VadEvent>),
    OutputMsg(PortSender<OutputMessage>),
    IpcEvent(PortSender<Event>),
    Signal(PortSender<()>),
}

impl OutputEndpoint {
    pub fn port_type(&self) -> PortType {
        match self {
            Self::Audio(_) => PortType::Audio,
            Self::VadEvent(_) => PortType::VadEvent,
            Self::OutputMsg(_) => PortType::OutputMsg,
            Self::IpcEvent(_) => PortType::IpcEvent,
            Self::Signal(_) => PortType::Signal,
        }
    }
}

/// Trait for nodes to receive their typed channel endpoints from the builder.
pub trait NodeWiring: Send + 'static {
    fn accept_input(&mut self, port: &str, endpoint: InputEndpoint) -> Result<()>;
    fn set_output(&mut self, port: &str, endpoint: OutputEndpoint) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{AudioFrame, VadEvent};

    #[test]
    fn input_endpoint_audio_variant() {
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<AudioFrame>();
        let ep = InputEndpoint::Audio(rx);
        assert!(matches!(ep, InputEndpoint::Audio(_)));
    }

    #[test]
    fn output_endpoint_audio_variant() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<AudioFrame>();
        let ep = OutputEndpoint::Audio(PortSender::Direct(tx));
        assert!(matches!(ep, OutputEndpoint::Audio(_)));
    }

    #[test]
    fn input_endpoint_port_type() {
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<AudioFrame>();
        let ep = InputEndpoint::Audio(rx);
        assert_eq!(ep.port_type(), PortType::Audio);
    }

    #[test]
    fn output_endpoint_port_type() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<VadEvent>();
        let ep = OutputEndpoint::VadEvent(PortSender::Direct(tx));
        assert_eq!(ep.port_type(), PortType::VadEvent);
    }
}
