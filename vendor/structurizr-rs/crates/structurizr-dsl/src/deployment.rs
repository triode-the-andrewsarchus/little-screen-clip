//! Deployment selection over the shared borrowed model inventory.

use std::collections::{HashMap, HashSet};
use structurizr_core::deployment::{DeploymentElement, DeploymentEntry};
use structurizr_core::prelude::*;

use crate::ast::{DeploymentViewNode, IncludeExclude, SelectionOperation};
use crate::error::{Error, ParseError, Result};
use crate::parser::build_view_properties;

pub(crate) fn build_view(
    model: &Model,
    source: DeploymentViewNode,
    identifiers: &HashMap<String, ElementId>,
    environments: &HashMap<String, String>,
) -> Result<DeploymentView> {
    let scope = source
        .scope
        .as_ref()
        .map(|name| {
            identifiers
                .get(name)
                .copied()
                .filter(|id| matches!(model.find_element(*id), Some(ElementRef::SoftwareSystem(_))))
                .ok_or_else(|| invalid(format!("Unknown or invalid deployment scope '{name}'")))
        })
        .transpose()?;
    let environment = environments.get(&source.environment).ok_or_else(|| {
        invalid(format!(
            "Unknown deployment environment '{}'",
            source.environment
        ))
    })?;
    let entries: Vec<_> = model
        .deployment_elements()
        .into_iter()
        .filter(|entry| entry.environment == environment)
        .collect();
    let parents: HashMap<_, _> = entries
        .iter()
        .filter_map(|entry| entry.parent_id.map(|parent| (entry.element.id(), parent)))
        .collect();
    let (mut properties, operations) =
        build_view_properties(source.properties, format!("Deployment-{environment}"));
    let mut selection = Selection {
        model,
        scope,
        entries: &entries,
        parents: &parents,
        identifiers,
        selected: HashSet::new(),
    };
    for operation in operations {
        let (terms, adding) = match operation {
            SelectionOperation::Include(terms) => (terms, true),
            SelectionOperation::Exclude(terms) => (terms, false),
        };
        for term in terms {
            selection.apply_term(term, adding)?;
        }
    }
    validate_abstraction(model, &entries, &selection.selected)?;
    properties.elements = entries
        .iter()
        .filter(|entry| selection.selected.contains(&entry.element.id()))
        .map(|entry| ElementView::new(entry.element.id()))
        .collect();
    properties.relationships = model
        .relationships
        .iter()
        .filter(|relationship| {
            selection.selected.contains(&relationship.source_id)
                && selection.selected.contains(&relationship.destination_id)
        })
        .map(|relationship| RelationshipView::new(relationship.id))
        .collect();
    Ok(DeploymentView {
        properties,
        software_system_id: scope,
        environment: environment.clone(),
    })
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Parse(ParseError {
        message: message.into(),
        location: None,
    })
}

fn is_descendant(id: ElementId, root: ElementId, parents: &HashMap<ElementId, ElementId>) -> bool {
    let mut current = Some(id);
    while let Some(id) = current {
        if id == root {
            return true;
        }
        current = parents.get(&id).copied();
    }
    false
}

fn add_with_parents(
    id: ElementId,
    parents: &HashMap<ElementId, ElementId>,
    selected: &mut HashSet<ElementId>,
) {
    let mut current = Some(id);
    while let Some(id) = current {
        selected.insert(id);
        current = parents.get(&id).copied();
    }
}

fn default_members(
    model: &Model,
    scope: Option<ElementId>,
    entries: &[DeploymentEntry<'_>],
    root: Option<ElementId>,
    parents: &HashMap<ElementId, ElementId>,
    selected: &HashSet<ElementId>,
) -> Vec<ElementId> {
    let in_branch = |entry: &&DeploymentEntry<'_>| {
        root.is_none_or(|root| is_descendant(entry.element.id(), root, parents))
    };
    let chosen_containers: HashSet<_> = entries
        .iter()
        .filter(|entry| selected.contains(&entry.element.id()))
        .filter_map(|entry| match entry.element {
            DeploymentElement::ContainerInstance(instance) => {
                model.ancestor_ids(instance.container_id).last().copied()
            }
            _ => None,
        })
        .collect();
    // Previously selected instances keep their abstraction. Within this branch,
    // system placements take precedence only when no child container was chosen.
    let systems: HashSet<_> = entries
        .iter()
        .filter_map(|entry| match entry.element {
            DeploymentElement::SoftwareSystemInstance(instance)
                if selected.contains(&instance.id)
                    || (in_branch(&entry)
                        && Some(instance.software_system_id) != scope
                        && !chosen_containers.contains(&instance.software_system_id)) =>
            {
                Some(instance.software_system_id)
            }
            _ => None,
        })
        .collect();
    entries
        .iter()
        .filter(in_branch)
        .filter(|entry| match entry.element {
            DeploymentElement::Node(_) => false,
            DeploymentElement::Infrastructure(_) => true,
            DeploymentElement::SoftwareSystemInstance(instance) => {
                Some(instance.software_system_id) != scope
                    && !chosen_containers.contains(&instance.software_system_id)
            }
            DeploymentElement::ContainerInstance(instance) => {
                let system = model.ancestor_ids(instance.container_id).last().copied();
                (scope.is_none() || scope == system)
                    && !system.is_some_and(|id| systems.contains(&id))
            }
        })
        .map(|entry| entry.element.id())
        .collect()
}

/// One ordered selection session owns its membership and borrowed view context.
struct Selection<'a> {
    model: &'a Model,
    scope: Option<ElementId>,
    entries: &'a [DeploymentEntry<'a>],
    parents: &'a HashMap<ElementId, ElementId>,
    identifiers: &'a HashMap<String, ElementId>,
    selected: HashSet<ElementId>,
}

impl Selection<'_> {
    fn apply_term(&mut self, term: IncludeExclude, adding: bool) -> Result<()> {
        let id = match term {
            IncludeExclude::All => None,
            IncludeExclude::Identifier(name) | IncludeExclude::Expression(name) => {
                Some(*self.identifiers.get(&name).ok_or_else(|| {
                    invalid(format!(
                        "Unknown deployment element or unsupported expression '{name}'"
                    ))
                })?)
            }
        };
        let entry = id
            .map(|id| {
                self.entries
                    .iter()
                    .find(|entry| entry.element.id() == id)
                    .ok_or_else(|| invalid("Element is not in the selected deployment environment"))
            })
            .transpose()?;
        if !adding {
            self.selected.retain(|member| {
                id.is_some_and(|root| !is_descendant(*member, root, self.parents))
            });
            return Ok(());
        }
        match entry.map(|entry| entry.element) {
            None | Some(DeploymentElement::Node(_)) => {
                for member in default_members(
                    self.model,
                    self.scope,
                    self.entries,
                    id,
                    self.parents,
                    &self.selected,
                ) {
                    add_with_parents(member, self.parents, &mut self.selected);
                }
            }
            Some(DeploymentElement::SoftwareSystemInstance(instance))
                if Some(instance.software_system_id) == self.scope =>
            {
                return Err(invalid(
                    "The scoped software system cannot be included as an instance",
                ));
            }
            Some(element) => add_with_parents(element.id(), self.parents, &mut self.selected),
        }
        Ok(())
    }
}

fn validate_abstraction(
    model: &Model,
    entries: &[DeploymentEntry<'_>],
    selected: &HashSet<ElementId>,
) -> Result<()> {
    let systems: HashSet<_> = entries
        .iter()
        .filter(|entry| selected.contains(&entry.element.id()))
        .filter_map(|entry| match entry.element {
            DeploymentElement::SoftwareSystemInstance(instance) => {
                Some(instance.software_system_id)
            }
            _ => None,
        })
        .collect();
    for entry in entries
        .iter()
        .filter(|entry| selected.contains(&entry.element.id()))
    {
        if let DeploymentElement::ContainerInstance(instance) = entry.element {
            if model
                .ancestor_ids(instance.container_id)
                .iter()
                .any(|id| systems.contains(id))
            {
                return Err(invalid(
                    "A deployment view cannot contain both system and child container instances",
                ));
            }
        }
    }
    Ok(())
}
