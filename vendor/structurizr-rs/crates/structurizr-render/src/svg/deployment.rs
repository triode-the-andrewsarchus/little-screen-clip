//! Deployment membership and containment, using the existing sibling layout.
use super::*;
use structurizr_core::deployment::{DeploymentElement, DeploymentEntry};
use structurizr_core::model::{ElementRef, Model, Relationship};

const INSET: f64 = 45.0;
const HEADER: f64 = 160.0;

type Parents = HashMap<String, Option<String>>;

pub(super) struct Diagram {
    pub nodes: Vec<LayoutNode>,
    pub elements: Vec<ElementInfo>,
    pub relationships: Vec<Relationship>,
    pub direction: structurizr_core::view::AutoLayoutDirection,
}

pub(super) fn prepare(workspace: &Workspace, view: &DeploymentView) -> Result<Diagram> {
    let model = workspace.model();
    let inventory = model.deployment_elements();
    let inventory: HashMap<_, _> = inventory
        .iter()
        .map(|entry| (entry.element.id(), entry))
        .collect();
    let entries = selected_entries(model, view, &inventory)?;
    let selected: HashSet<_> = entries.iter().map(|entry| entry.element.id()).collect();
    let elements: Vec<_> = entries
        .iter()
        .map(|entry| info(model, *entry))
        .collect::<Result<_>>()?;
    let mut parents = HashMap::new();
    for entry in &entries {
        let mut parent = entry.parent_id;
        // A hidden intermediate node does not invent a new host. Retain the
        // nearest selected ancestor from the actual deployment hierarchy.
        while let Some(id) = parent.filter(|id| !selected.contains(id)) {
            parent = inventory.get(&id).and_then(|entry| entry.parent_id);
        }
        parents.insert(
            entry.element.id().to_string(),
            parent.map(|id| id.to_string()),
        );
    }
    let requested: HashSet<_> = view.properties.relationships.iter().map(|r| r.id).collect();
    let relationships: Vec<_> = model
        .relationships
        .iter()
        .filter(|r| {
            requested.contains(&r.id)
                && selected.contains(&r.source_id)
                && selected.contains(&r.destination_id)
        })
        .cloned()
        .collect();
    let edges: Vec<_> = relationships
        .iter()
        .map(|r| LayoutEdge {
            source: r.source_id.to_string(),
            target: r.destination_id.to_string(),
        })
        .collect();
    let resolver = StyleResolver::new(workspace.styles());
    let sizes: HashMap<_, _> = elements
        .iter()
        .map(|element| {
            let style =
                resolver.resolve_element(&element.tags, element.element_type.to_element_kind());
            (
                element.id.to_string(),
                Size {
                    width: style.width as f64,
                    height: style.height as f64,
                },
            )
        })
        .collect();
    let mut children: HashMap<Option<String>, Vec<String>> = HashMap::new();
    for element in &elements {
        let id = element.id.to_string();
        children.entry(parents[&id].clone()).or_default().push(id);
    }
    let grid =
        GridLayout::for_view_type(ViewType::Deployment, view.properties.auto_layout.as_ref());
    let mut nodes = layout_group(None, &children, &sizes, &edges, &grid);
    explicit_positions(
        &mut nodes,
        &parents,
        &extract_explicit_positions(&view.properties),
    );
    Ok(Diagram {
        nodes,
        elements,
        relationships,
        direction: grid.direction,
    })
}

/// Resolve and validate the concrete placement selection before any layout.
fn selected_entries<'a>(
    model: &Model,
    view: &DeploymentView,
    inventory: &HashMap<ElementId, &DeploymentEntry<'a>>,
) -> Result<Vec<DeploymentEntry<'a>>> {
    if view
        .software_system_id
        .is_some_and(|id| !matches!(model.find_element(id), Some(ElementRef::SoftwareSystem(_))))
    {
        return Err(invalid("Deployment view scope is not a software system"));
    }
    let mut selected = HashSet::new();
    let mut entries = Vec::new();
    for requested in &view.properties.elements {
        if !selected.insert(requested.id) {
            return Err(invalid("Duplicate deployment placement in view"));
        }
        let entry = inventory
            .get(&requested.id)
            .ok_or_else(|| crate::error::Error::ElementNotFound(requested.id.to_string()))?;
        if entry.environment != view.environment {
            return Err(invalid(
                "Deployment placement is outside the view environment",
            ));
        }
        if matches!(entry.element, DeploymentElement::SoftwareSystemInstance(instance)
            if Some(instance.software_system_id) == view.software_system_id)
        {
            return Err(invalid(
                "The scoped system cannot also appear as a system instance",
            ));
        }
        entries.push(**entry);
    }
    validate_abstraction(model, &entries)?;
    Ok(entries)
}

fn invalid(message: &str) -> crate::error::Error {
    crate::error::Error::Render(message.into())
}

fn validate_abstraction(model: &Model, entries: &[DeploymentEntry<'_>]) -> Result<()> {
    let systems: HashSet<_> = entries
        .iter()
        .filter_map(|entry| match entry.element {
            DeploymentElement::SoftwareSystemInstance(instance) => {
                Some(instance.software_system_id)
            }
            _ => None,
        })
        .collect();
    for entry in entries {
        if let DeploymentElement::ContainerInstance(instance) = entry.element {
            if model
                .ancestor_ids(instance.container_id)
                .iter()
                .any(|id| systems.contains(id))
            {
                return Err(invalid(
                    "Deployment view mixes system and child container instances",
                ));
            }
        }
    }
    Ok(())
}

fn info(model: &Model, entry: DeploymentEntry<'_>) -> Result<ElementInfo> {
    let (properties, technology, element_type) = match entry.element {
        DeploymentElement::Node(node) => (
            &node.properties,
            &node.technology,
            ElementType::DeploymentNode,
        ),
        DeploymentElement::Infrastructure(node) => (
            &node.properties,
            &node.technology,
            ElementType::InfrastructureNode,
        ),
        _ => return instance_info(model, entry.element),
    };
    Ok(ElementInfo {
        id: properties.id,
        name: properties.name.clone(),
        description: properties.description.clone(),
        technology: technology.clone(),
        element_type,
        tags: properties.tags.clone(),
    })
}

fn instance_info(model: &Model, element: DeploymentElement<'_>) -> Result<ElementInfo> {
    let (base, description, tags, kind, instance_tag) = match element {
        DeploymentElement::ContainerInstance(instance) => (
            instance.container_id,
            &instance.description,
            &instance.tags,
            ElementType::ContainerInstance(instance.instance_id),
            "Container Instance",
        ),
        DeploymentElement::SoftwareSystemInstance(instance) => (
            instance.software_system_id,
            &instance.description,
            &instance.tags,
            ElementType::SoftwareSystemInstance(instance.instance_id),
            "Software System Instance",
        ),
        _ => return Err(invalid("Expected a deployed instance")),
    };
    let mut info = ElementInfo::static_element(model, base, ViewType::SystemLandscape, Some(base))?;
    if !matches!(
        (info.element_type, kind),
        (ElementType::Container, ElementType::ContainerInstance(_))
            | (
                ElementType::SoftwareSystem,
                ElementType::SoftwareSystemInstance(_)
            )
    ) {
        return Err(invalid(
            "Deployment instance references the wrong static element type",
        ));
    }
    info.id = element.id();
    info.element_type = kind;
    info.description = description.clone().or(info.description);
    for tag in std::iter::once(instance_tag.to_owned()).chain(tags.iter().cloned()) {
        if !info.tags.contains(&tag) {
            info.tags.push(tag);
        }
    }
    Ok(info)
}

/// Place siblings with projected links, then move their already-sized subtrees.
fn layout_group(
    parent: Option<String>,
    children: &HashMap<Option<String>, Vec<String>>,
    sizes: &HashMap<String, Size>,
    edges: &[LayoutEdge],
    grid: &GridLayout,
) -> Vec<LayoutNode> {
    let Some(ids) = children.get(&parent) else {
        return Vec::new();
    };
    let mut subtrees = Vec::new();
    for id in ids {
        let mut descendants = layout_group(Some(id.clone()), children, sizes, edges, grid);
        let mut size = sizes[id];
        if !descendants.is_empty() {
            let bounds = node_bounds(&descendants);
            shift(
                &mut descendants,
                INSET - bounds.min_x,
                HEADER - bounds.min_y,
            );
            size.width = size.width.max(bounds.width() + INSET * 2.0);
            size.height = size.height.max(bounds.height() + HEADER + INSET);
        }
        let mut nodes = vec![LayoutNode {
            id: id.clone(),
            position: Position { x: 0.0, y: 0.0 },
            size,
            rank: 0,
        }];
        nodes.extend(descendants);
        subtrees.push(nodes);
    }
    let branches: HashMap<_, _> = subtrees
        .iter()
        .flat_map(|nodes| {
            nodes
                .iter()
                .map(|node| (node.id.as_str(), nodes[0].id.as_str()))
        })
        .collect();
    let projected: Vec<_> = edges
        .iter()
        .filter_map(|edge| {
            let source = branches.get(edge.source.as_str())?;
            let target = branches.get(edge.target.as_str())?;
            (source != target).then(|| LayoutEdge {
                source: (*source).to_owned(),
                target: (*target).to_owned(),
            })
        })
        .collect();
    let node_sizes: Vec<_> = subtrees
        .iter()
        .map(|nodes| (nodes[0].id.clone(), nodes[0].size))
        .collect();
    // The existing rank placer uses default dimensions for inter-rank spacing.
    // Supply the largest actual sibling size so nested hosts cannot overlap.
    let spacing = GridLayout {
        direction: grid.direction,
        rank_separation: grid.rank_separation,
        node_separation: grid.node_separation,
        default_width: node_sizes
            .iter()
            .map(|(_, s)| s.width)
            .fold(grid.default_width, f64::max),
        default_height: node_sizes
            .iter()
            .map(|(_, s)| s.height)
            .fold(grid.default_height, f64::max),
    };
    let positions: HashMap<_, _> = spacing
        .layout_sugiyama(ids, &node_sizes, &projected)
        .nodes
        .into_iter()
        .map(|node| (node.id, (node.position.x, node.position.y, node.rank)))
        .collect();
    subtrees
        .into_iter()
        .flat_map(|mut nodes| {
            let (x, y, rank) = positions[&nodes[0].id];
            shift(&mut nodes, x, y);
            for node in &mut nodes {
                node.rank += rank;
            }
            nodes
        })
        .collect()
}

fn node_bounds(nodes: &[LayoutNode]) -> ContentBounds {
    let mut bounds = ContentBounds::new();
    for node in nodes {
        bounds.expand(
            node.position.x,
            node.position.y,
            node.size.width,
            node.size.height,
        );
    }
    bounds
}

fn shift(nodes: &mut [LayoutNode], x: f64, y: f64) {
    for node in nodes {
        node.position.x += x;
        node.position.y += y;
    }
}

fn descendant(id: &str, ancestor: &str, parents: &Parents) -> bool {
    let mut current = Some(id);
    while let Some(id) = current {
        if id == ancestor {
            return true;
        }
        current = parents.get(id).and_then(|parent| parent.as_deref());
    }
    false
}

fn explicit_positions(
    nodes: &mut [LayoutNode],
    parents: &Parents,
    positions: &HashMap<String, (i32, i32)>,
) {
    // Layout order is parent before child. Moving an authored host carries its
    // descendants; a child's own absolute coordinates are then applied afterward.
    for index in 0..nodes.len() {
        let id = nodes[index].id.clone();
        if let Some(&(x, y)) = positions.get(&id) {
            let dx = x as f64 - nodes[index].position.x;
            let dy = y as f64 - nodes[index].position.y;
            for node in nodes
                .iter_mut()
                .filter(|node| descendant(&node.id, &id, parents))
            {
                node.position.x += dx;
                node.position.y += dy;
            }
        }
    }
    // Keep a real container around manually moved children without changing the
    // authored child positions. A host can grow/shift only as required to fit.
    for index in (0..nodes.len()).rev() {
        let id = &nodes[index].id;
        let direct: Vec<_> = nodes
            .iter()
            .filter(|node| parents.get(&node.id).and_then(|p| p.as_ref()) == Some(id))
            .cloned()
            .collect();
        if direct.is_empty() {
            continue;
        }
        let child = node_bounds(&direct);
        let node = &mut nodes[index];
        let right = (node.position.x + node.size.width).max(child.max_x + INSET);
        let bottom = (node.position.y + node.size.height).max(child.max_y + INSET);
        node.position.x = node.position.x.min(child.min_x - INSET);
        node.position.y = node.position.y.min(child.min_y - HEADER);
        node.size.width = right - node.position.x;
        node.size.height = bottom - node.position.y;
    }
}

/// A self link must leave the node's fill, otherwise the shared foreground
/// painting would hide it. Keep the loop within the containing host's inset.
pub(super) fn self_loop(node: &LayoutNode) -> EdgePath {
    let right = node.position.x + node.size.width;
    let lower = node.position.y + node.size.height * 0.65;
    let upper = node.position.y + node.size.height * 0.35;
    EdgePath::Orthogonal {
        waypoints: vec![
            crate::layout::Point::new(right, lower),
            crate::layout::Point::new(right + 30.0, lower),
            crate::layout::Point::new(right + 30.0, upper),
            crate::layout::Point::new(right, upper),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structurizr_core::model::{
        Container, ContainerInstance, DeploymentNode, InfrastructureNode, SoftwareSystem,
        SoftwareSystemInstance,
    };
    use structurizr_core::style::{ElementStyle, RelationshipStyle};
    use structurizr_core::view::{AutoLayout, AutoLayoutDirection, ElementView};

    struct Fixture {
        workspace: Workspace,
        view: DeploymentView,
        region: ElementId,
        host: ElementId,
        peer: ElementId,
        first: ElementId,
        second: ElementId,
        infra: ElementId,
        base: ElementId,
        system: ElementId,
    }

    fn fixture() -> Fixture {
        let mut workspace = Workspace::new("Deployment", "Selected placements");
        let mut system = SoftwareSystem::new("Shop");
        let mut app = Container::new("API").with_technology("Rust");
        app.properties.tags.push("Service".into());
        let base = app.id();
        system.containers.push(app);
        let system_id = system.id();
        workspace.model_mut().software_systems.push(system);
        let placement = |number| ContainerInstance {
            id: ElementId::new(),
            container_id: base,
            description: Some(format!("Replica {number}")),
            instance_id: number,
            tags: vec!["Placement".into()],
            properties: HashMap::new(),
            health_checks: Vec::new(),
        };
        let mut region = DeploymentNode::new("Region");
        region.environment = Some("Production".into());
        let mut host = DeploymentNode::new("Host A");
        let mut peer = DeploymentNode::new("Host B");
        host.container_instances.push(placement(1));
        peer.container_instances.push(placement(2));
        let infra = InfrastructureNode::new("Router");
        let infra_id = infra.id();
        region.infrastructure_nodes.push(infra);
        let first = host.container_instances[0].id;
        let second = peer.container_instances[0].id;
        let (region_id, host_id, peer_id) = (region.id(), host.id(), peer.id());
        region.children.extend([host, peer]);
        workspace.model_mut().deployment_nodes.push(region);
        let mut hidden = DeploymentNode::new("Hidden environment");
        hidden.environment = Some("Testing".into());
        hidden.container_instances.push(placement(3));
        workspace.model_mut().deployment_nodes.push(hidden);
        let mut view = DeploymentView::for_system("Production", system_id, "Production");
        // Deliberately reverse model order: containment must not depend on the
        // selection's paint order or implicitly re-include omitted objects.
        for id in [second, first, peer_id, host_id, region_id, infra_id] {
            view.properties.add_element(id);
        }
        let model = workspace.model_mut();
        for (from, to, label) in [
            (first, second, "Replica traffic"),
            (infra_id, first, "Routes"),
            (first, first, "Self check"),
        ] {
            let id = model.add_relationship(from, to, label, Some("TLS".into()));
            view.properties.add_relationship(id);
        }
        model.add_relationship(first, second, "Excluded relationship", None);
        workspace.styles_mut().elements.push(ElementStyle {
            tag: "Placement".into(),
            background: Some("#b34825".into()),
            width: Some(510),
            ..Default::default()
        });
        workspace
            .styles_mut()
            .relationships
            .push(RelationshipStyle {
                tag: "Relationship".into(),
                color: Some("#ad1973".into()),
                ..Default::default()
            });
        Fixture {
            workspace,
            view,
            region: region_id,
            host: host_id,
            peer: peer_id,
            first,
            second,
            infra: infra_id,
            base,
            system: system_id,
        }
    }

    fn node(diagram: &Diagram, id: ElementId) -> &LayoutNode {
        diagram
            .nodes
            .iter()
            .find(|node| node.id == id.to_string())
            .unwrap()
    }

    fn contains(parent: &LayoutNode, child: &LayoutNode) {
        assert!(child.position.x >= parent.position.x + INSET - 0.01);
        assert!(child.position.y >= parent.position.y + HEADER - 0.01);
        assert!(
            child.position.x + child.size.width
                <= parent.position.x + parent.size.width - INSET + 0.01
        );
        assert!(
            child.position.y + child.size.height
                <= parent.position.y + parent.size.height - INSET + 0.01
        );
    }

    fn disjoint(a: &LayoutNode, b: &LayoutNode) {
        assert!(
            a.position.x + a.size.width <= b.position.x
                || b.position.x + b.size.width <= a.position.x
                || a.position.y + a.size.height <= b.position.y
                || b.position.y + b.size.height <= a.position.y,
            "sibling rectangles overlap: {a:?} {b:?}"
        );
    }

    #[test]
    fn selected_instances_keep_identity_metadata_and_real_containment_in_all_directions() {
        let mut f = fixture();
        for direction in [
            AutoLayoutDirection::TopBottom,
            AutoLayoutDirection::BottomTop,
            AutoLayoutDirection::LeftRight,
            AutoLayoutDirection::RightLeft,
        ] {
            f.view.properties.auto_layout = Some(AutoLayout {
                direction,
                ..Default::default()
            });
            let diagram = prepare(&f.workspace, &f.view).unwrap();
            assert_eq!(diagram.elements.len(), 6);
            assert_eq!(diagram.relationships.len(), 3);
            assert!(!diagram.elements.iter().any(|element| element.id == f.base));
            contains(node(&diagram, f.region), node(&diagram, f.host));
            contains(node(&diagram, f.region), node(&diagram, f.peer));
            contains(node(&diagram, f.region), node(&diagram, f.infra));
            contains(node(&diagram, f.host), node(&diagram, f.first));
            contains(node(&diagram, f.peer), node(&diagram, f.second));
            disjoint(node(&diagram, f.host), node(&diagram, f.peer));
            disjoint(node(&diagram, f.host), node(&diagram, f.infra));
            let first = diagram
                .elements
                .iter()
                .find(|element| element.id == f.first)
                .unwrap();
            assert_eq!(first.name, "API");
            assert_eq!(first.description.as_deref(), Some("Replica 1"));
            assert_eq!(first.technology.as_deref(), Some("Rust"));
            for tag in ["Container", "Container Instance", "Service", "Placement"] {
                assert!(first.tags.iter().any(|t| t == tag));
            }
            assert_eq!(node(&diagram, f.first).size.width, 510.0);
        }
    }

    #[test]
    fn boundaries_paint_before_actual_visible_links_and_leaf_foregrounds() {
        let f = fixture();
        let diagram = prepare(&f.workspace, &f.view).unwrap();
        let svg = SvgRenderer::new(0, 0)
            .render_deployment(&f.workspace, &f.view)
            .unwrap();
        let index = |id| svg.find(&format!("data-element-id=\"{id}\"")).unwrap();
        let edge_index = svg.find("class=\"relationship\"").unwrap();
        assert!(index(f.region) < index(f.host));
        assert!(index(f.host) < edge_index && index(f.peer) < edge_index);
        assert!(edge_index < index(f.first) && edge_index < index(f.second));
        assert!(svg.contains("#b34825") && svg.contains("#ad1973"));
        assert!(svg.contains("[Container Instance: 1]"));
        assert!(!svg.contains("Excluded relationship"));
        let obstacles: Vec<_> = diagram
            .nodes
            .iter()
            .filter(|n| {
                [f.first, f.second, f.infra]
                    .iter()
                    .any(|id| n.id == id.to_string())
            })
            .cloned()
            .collect();
        let path = OrthogonalRouter::with_nodes(
            OrthogonalConfig::for_direction(diagram.direction),
            &obstacles,
        )
        .route_edge(
            node(&diagram, f.first),
            node(&diagram, f.second),
            0,
            1,
            0,
            1,
        );
        let EdgePath::Orthogonal { waypoints } = path else {
            panic!("not orthogonal")
        };
        assert!(waypoints
            .windows(2)
            .any(|pair| (pair[0].x - pair[1].x).abs() + (pair[0].y - pair[1].y).abs() > 20.0));
        let region = node(&diagram, f.region);
        for point in waypoints {
            assert!(
                point.x >= region.position.x && point.x <= region.position.x + region.size.width
            );
            assert!(
                point.y >= region.position.y && point.y <= region.position.y + region.size.height
            );
        }
        let first = node(&diagram, f.first);
        let EdgePath::Orthogonal { waypoints } = self_loop(first) else {
            panic!("not orthogonal")
        };
        assert!(waypoints[1].x > first.position.x + first.size.width);
        assert!(waypoints[0].y != waypoints[3].y);
    }

    #[test]
    fn explicit_positions_move_subtrees_and_keep_manually_moved_children_contained() {
        let mut f = fixture();
        for element in &mut f.view.properties.elements {
            if element.id == f.host {
                element.x = Some(3000);
                element.y = Some(2000);
            }
            if element.id == f.first {
                element.x = Some(2500);
                element.y = Some(1800);
            }
        }
        let diagram = prepare(&f.workspace, &f.view).unwrap();
        let first = node(&diagram, f.first);
        assert_eq!((first.position.x, first.position.y), (2500.0, 1800.0));
        contains(node(&diagram, f.host), first);
        contains(node(&diagram, f.region), node(&diagram, f.host));
    }

    #[test]
    fn exact_empty_or_excluded_membership_never_regrows_the_model() {
        let mut f = fixture();
        f.view
            .properties
            .elements
            .retain(|element| [f.first, f.region].contains(&element.id));
        let diagram = prepare(&f.workspace, &f.view).unwrap();
        assert_eq!(diagram.elements.len(), 2);
        assert_eq!(
            diagram.relationships.len(),
            1,
            "only the selected self link survives"
        );
        contains(node(&diagram, f.region), node(&diagram, f.first));
        f.view.properties.elements.clear();
        let diagram = prepare(&f.workspace, &f.view).unwrap();
        assert!(diagram.nodes.is_empty() && diagram.relationships.is_empty());
        let svg = SvgRenderer::new(0, 0)
            .render_deployment(&f.workspace, &f.view)
            .unwrap();
        assert!(svg.contains("viewBox=\"0 0 100 100\""));
        assert!(!svg.contains("draggable-element"));
    }

    #[test]
    fn environments_scopes_instance_bases_and_abstraction_are_checked() {
        let mut f = fixture();
        f.view
            .properties
            .add_element(f.workspace.model().deployment_nodes[1].id());
        assert!(prepare(&f.workspace, &f.view).is_err());
        f.view.properties.elements.pop();
        f.view.software_system_id = Some(f.base);
        assert!(prepare(&f.workspace, &f.view).is_err());
        f.view.software_system_id = None;
        let placement = SoftwareSystemInstance {
            id: ElementId::new(),
            software_system_id: f.system,
            description: Some("External placement".into()),
            instance_id: 2,
            tags: vec!["Remote".into()],
            properties: HashMap::new(),
            health_checks: Vec::new(),
        };
        let id = placement.id;
        f.workspace.model_mut().deployment_nodes[0]
            .software_system_instances
            .push(placement);
        f.view.properties.elements = vec![ElementView::new(id)];
        let diagram = prepare(&f.workspace, &f.view).unwrap();
        assert_eq!(diagram.elements[0].name, "Shop");
        assert_eq!(
            diagram.elements[0].element_type,
            ElementType::SoftwareSystemInstance(2)
        );
        f.view.properties.add_element(f.first);
        assert!(prepare(&f.workspace, &f.view).is_err());
        f.view.properties.elements = vec![ElementView::new(id)];
        f.view.software_system_id = Some(f.system);
        assert!(prepare(&f.workspace, &f.view).is_err());
        f.view.software_system_id = None;
        f.workspace.model_mut().deployment_nodes[0].software_system_instances[0]
            .software_system_id = f.base;
        assert!(prepare(&f.workspace, &f.view).is_err());
    }
}
