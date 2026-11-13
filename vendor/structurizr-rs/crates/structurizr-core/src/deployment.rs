//! Borrowed deployment inventory shared by view selection and rendering.

use crate::model::{
    ContainerInstance, DeploymentNode, ElementId, InfrastructureNode, Model, SoftwareSystemInstance,
};

/// A deployment object together with its observed containment and environment.
#[derive(Debug, Clone, Copy)]
pub struct DeploymentEntry<'a> {
    /// The concrete node, infrastructure object or deployed instance.
    pub element: DeploymentElement<'a>,
    /// The immediate containing deployment node, if present.
    pub parent_id: Option<ElementId>,
    /// The nearest declared environment, or `Default` for an unscoped root.
    pub environment: &'a str,
}

/// References preserve instance identity separately from the static declaration.
#[derive(Debug, Clone, Copy)]
pub enum DeploymentElement<'a> {
    Node(&'a DeploymentNode),
    Infrastructure(&'a InfrastructureNode),
    ContainerInstance(&'a ContainerInstance),
    SoftwareSystemInstance(&'a SoftwareSystemInstance),
}

impl DeploymentElement<'_> {
    /// Return this placement's identity, never the referenced static identity.
    pub fn id(self) -> ElementId {
        match self {
            Self::Node(node) => node.id(),
            Self::Infrastructure(node) => node.id(),
            Self::ContainerInstance(instance) => instance.id,
            Self::SoftwareSystemInstance(instance) => instance.id,
        }
    }

    /// Return the static declaration instantiated by this placement, if any.
    pub fn base_id(self) -> Option<ElementId> {
        match self {
            Self::ContainerInstance(instance) => Some(instance.container_id),
            Self::SoftwareSystemInstance(instance) => Some(instance.software_system_id),
            _ => None,
        }
    }
}

impl Model {
    /// Inventory deployment objects in deterministic containment order.
    ///
    /// Descendants inherit their containing node's environment unless they
    /// explicitly declare another one. Every item borrows the original model;
    /// no static declaration or instance metadata is copied or substituted.
    pub fn deployment_elements(&self) -> Vec<DeploymentEntry<'_>> {
        let mut entries = Vec::new();
        collect_nodes(&self.deployment_nodes, None, "Default", &mut entries);
        entries
    }
}

fn collect_nodes<'a>(
    nodes: &'a [DeploymentNode],
    parent_id: Option<ElementId>,
    environment: &'a str,
    entries: &mut Vec<DeploymentEntry<'a>>,
) {
    for node in nodes {
        let environment = node.environment.as_deref().unwrap_or(environment);
        entries.push(DeploymentEntry {
            element: DeploymentElement::Node(node),
            parent_id,
            environment,
        });
        entries.extend(
            node.infrastructure_nodes
                .iter()
                .map(DeploymentElement::Infrastructure)
                .chain(
                    node.container_instances
                        .iter()
                        .map(DeploymentElement::ContainerInstance),
                )
                .chain(
                    node.software_system_instances
                        .iter()
                        .map(DeploymentElement::SoftwareSystemInstance),
                )
                .map(|element| DeploymentEntry {
                    element,
                    parent_id: Some(node.id()),
                    environment,
                }),
        );
        collect_nodes(&node.children, Some(node.id()), environment, entries);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ElementProperties;
    use std::collections::HashMap;

    #[test]
    fn inventory_retains_containment_inherited_environments_and_instance_identity() {
        let mut root = DeploymentNode::new("Region");
        root.environment = Some("Production".into());
        let mut host = DeploymentNode::new("Host");
        let balancer = InfrastructureNode::new("Balancer");
        let balancer_id = balancer.id();
        host.infrastructure_nodes.push(balancer);
        let base = ElementProperties::new("Service").id;
        let placement = ElementId::new();
        host.container_instances.push(ContainerInstance {
            id: placement,
            container_id: base,
            description: None,
            instance_id: 1,
            tags: Vec::new(),
            properties: HashMap::new(),
            health_checks: Vec::new(),
        });
        let nested_id = host.id();
        root.children.push(host);
        let root_id = root.id();
        let mut override_node = DeploymentNode::new("Isolated");
        override_node.environment = Some("Testing".into());
        root.children.push(override_node);
        let mut model = Model::default();
        model.deployment_nodes.push(root);
        model
            .deployment_nodes
            .push(DeploymentNode::new("Legacy default"));
        let entries = model.deployment_elements();
        assert_eq!(entries.len(), 6);
        assert_eq!(entries[0].parent_id, None);
        assert_eq!(entries[1].parent_id, Some(root_id));
        assert_eq!(entries[2].parent_id, Some(nested_id));
        assert_eq!(entries[2].element.id(), balancer_id);
        assert_eq!(entries[2].element.base_id(), None);
        assert_eq!(entries[3].parent_id, Some(nested_id));
        assert_eq!(entries[3].element.id(), placement);
        assert_eq!(entries[3].element.base_id(), Some(base));
        assert!(entries[..4]
            .iter()
            .all(|entry| entry.environment == "Production"));
        assert_eq!(entries[4].environment, "Testing");
        assert_eq!(entries[5].environment, "Default");
        assert_eq!(entries[5].parent_id, None);
        assert_eq!(model.deployment_elements()[3].element.id(), placement);
    }
}
