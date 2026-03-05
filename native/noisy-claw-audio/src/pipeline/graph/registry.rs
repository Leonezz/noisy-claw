use anyhow::Result;

use super::node::PipelineNode;
use super::types::{PortDescriptor, PropertyDescriptor};

/// Auto-registered node factory entry. Each node module submits one via inventory::submit!
pub struct NodeFactoryEntry {
    pub node_type: &'static str,
    pub description: &'static str,
    pub factory: fn(props: &serde_json::Value) -> Result<Box<dyn PipelineNode>>,
    pub ports: fn() -> Vec<PortDescriptor>,
    pub property_descriptors: fn() -> Vec<PropertyDescriptor>,
}

inventory::collect!(NodeFactoryEntry);

/// Registry of all available node types. Backed by inventory — no manual registration.
pub struct NodeRegistry;

impl NodeRegistry {
    /// Iterate over all registered node factory entries.
    pub fn iter() -> impl Iterator<Item = &'static NodeFactoryEntry> {
        inventory::iter::<NodeFactoryEntry>()
    }

    /// Find a factory entry by node type name.
    pub fn find(node_type: &str) -> Option<&'static NodeFactoryEntry> {
        Self::iter().find(|e| e.node_type == node_type)
    }

    /// Create a node instance by type name and properties.
    pub fn create(node_type: &str, props: &serde_json::Value) -> Result<Box<dyn PipelineNode>> {
        let entry = Self::find(node_type)
            .ok_or_else(|| anyhow::anyhow!("unknown node type: {node_type}"))?;
        (entry.factory)(props)
    }

    /// List all registered node type names.
    pub fn node_types() -> Vec<&'static str> {
        Self::iter().map(|e| e.node_type).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_iter_returns_submitted_entries() {
        let count = NodeRegistry::iter().count();
        assert!(count >= 8, "expected at least 8 registered nodes, got {count}");
    }

    #[test]
    fn registry_find_nonexistent_returns_none() {
        assert!(NodeRegistry::find("nonexistent_node_type").is_none());
    }

    #[test]
    fn registry_node_types_returns_vec() {
        let types = NodeRegistry::node_types();
        assert!(types.len() >= 8, "expected at least 8 node types, got {}", types.len());
    }

    #[test]
    fn registry_finds_all_pipeline_nodes() {
        let expected = ["capture", "aec", "vad", "stt", "tts", "output", "topic", "ipc_sink"];
        for name in expected {
            assert!(
                NodeRegistry::find(name).is_some(),
                "node type '{name}' not found in registry"
            );
        }
    }

    #[test]
    fn registry_factory_creates_node() {
        let node = NodeRegistry::create("capture", &serde_json::json!({}));
        assert!(node.is_ok(), "failed to create capture node: {:?}", node.err());
    }
}
