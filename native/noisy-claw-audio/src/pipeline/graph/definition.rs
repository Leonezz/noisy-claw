use std::collections::HashMap;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// JSON-serializable pipeline definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PipelineDefinition {
    pub name: String,
    pub nodes: Vec<NodeDefinition>,
    pub links: Vec<LinkDefinition>,
    #[serde(default)]
    pub modes: HashMap<String, HashMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(default = "default_properties")]
    pub properties: serde_json::Value,
}

fn default_properties() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkDefinition {
    pub from: String,
    pub to: String,
}

/// Parse a port reference like "mic:audio_out" into (node_name, port_name).
pub fn parse_port_ref(s: &str) -> Result<(&str, &str)> {
    let mut parts = s.splitn(2, ':');
    let node = parts.next().unwrap_or("");
    let port = parts.next().unwrap_or("");
    if node.is_empty() || port.is_empty() {
        return Err(anyhow!("invalid port reference: '{s}' (expected 'node:port')"));
    }
    Ok((node, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"{
        "name": "test-pipeline",
        "nodes": [
            { "name": "mic", "type": "capture", "properties": { "device": "default" } },
            { "name": "vad", "type": "vad", "properties": { "threshold": 0.5 } }
        ],
        "links": [
            { "from": "mic:audio_out", "to": "vad:audio_in" }
        ],
        "modes": {
            "conversation": {
                "vad": { "threshold": 0.5 }
            },
            "meeting": {
                "vad": { "threshold": 0.3 }
            }
        }
    }"#;

    #[test]
    fn deserialize_pipeline_definition() {
        let def: PipelineDefinition = serde_json::from_str(SAMPLE_JSON).unwrap();
        assert_eq!(def.name, "test-pipeline");
        assert_eq!(def.nodes.len(), 2);
        assert_eq!(def.links.len(), 1);
        assert_eq!(def.modes.len(), 2);
    }

    #[test]
    fn node_definition_fields() {
        let def: PipelineDefinition = serde_json::from_str(SAMPLE_JSON).unwrap();
        let mic = &def.nodes[0];
        assert_eq!(mic.name, "mic");
        assert_eq!(mic.node_type, "capture");
        assert_eq!(mic.properties["device"], "default");
    }

    #[test]
    fn link_definition_fields() {
        let def: PipelineDefinition = serde_json::from_str(SAMPLE_JSON).unwrap();
        let link = &def.links[0];
        assert_eq!(link.from, "mic:audio_out");
        assert_eq!(link.to, "vad:audio_in");
    }

    #[test]
    fn parse_port_ref_valid() {
        let (node, port) = parse_port_ref("mic:audio_out").unwrap();
        assert_eq!(node, "mic");
        assert_eq!(port, "audio_out");
    }

    #[test]
    fn parse_port_ref_invalid() {
        assert!(parse_port_ref("no_colon").is_err());
        assert!(parse_port_ref(":no_node").is_err());
        assert!(parse_port_ref("no_port:").is_err());
    }

    #[test]
    fn mode_definition_overrides() {
        let def: PipelineDefinition = serde_json::from_str(SAMPLE_JSON).unwrap();
        let meeting = &def.modes["meeting"];
        assert_eq!(meeting["vad"]["threshold"], 0.3);
    }

    #[test]
    fn serialize_roundtrip() {
        let def: PipelineDefinition = serde_json::from_str(SAMPLE_JSON).unwrap();
        let json = serde_json::to_string_pretty(&def).unwrap();
        let def2: PipelineDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(def.name, def2.name);
        assert_eq!(def.nodes.len(), def2.nodes.len());
    }

    #[test]
    fn default_modes_empty() {
        let json = r#"{ "name": "simple", "nodes": [], "links": [] }"#;
        let def: PipelineDefinition = serde_json::from_str(json).unwrap();
        assert!(def.modes.is_empty());
    }
}
