//! Integration tests for DSL directives.

use structurizr_dsl::parse;

#[test]
fn test_multiple_directives_combined() {
    let dsl = r##"
workspace "E-Commerce Platform" "An example with all directives" {
    !const COMPANY "ShopCo"
    !const DATABASE "PostgreSQL"
    !const API_PROTOCOL "REST/HTTPS"
    !impliedRelationships true
    !docs "documentation"
    !adrs "architecture-decisions"

    model {
        customer = person "${COMPANY} Customer" "A customer of ${COMPANY}"
        admin = person "${COMPANY} Admin" "System administrator"

        webapp = softwareSystem "Web Application" "Allows customers to browse and purchase products"
        database = softwareSystem "Database" "Stores product and order data using ${DATABASE}"
        payment = softwareSystem "Payment Gateway" "Processes payments"

        customer -> webapp "Browses and purchases products"
        admin -> webapp "Manages products and orders"
        webapp -> database "Reads from and writes to" "${DATABASE}"
        webapp -> payment "Processes payments via" "${API_PROTOCOL}"
    }

    views {
        systemContext webapp "SystemContext" "System Context for Web Application" {
            include *
            autoLayout
        }

        styles {
            element "Person" {
                shape Person
                background "#08427b"
                color "#ffffff"
            }
            element "Software System" {
                background "#1168bd"
                color "#ffffff"
            }
        }
    }
}
"##;

    let workspace = parse(dsl).unwrap();

    // Test constant substitution
    assert_eq!(workspace.model().people.len(), 2);
    assert_eq!(workspace.model().people[0].name(), "ShopCo Customer");
    assert_eq!(workspace.model().people[1].name(), "ShopCo Admin");

    assert_eq!(workspace.model().software_systems.len(), 3);
    assert_eq!(
        workspace.model().software_systems[1].properties.description,
        Some("Stores product and order data using PostgreSQL".to_string())
    );

    // Test relationships with constants
    let rel_with_tech = workspace.model().relationships.iter()
        .find(|r| r.technology == Some("PostgreSQL".to_string()));
    assert!(rel_with_tech.is_some(), "Should have relationship with PostgreSQL technology");

    let rel_with_api = workspace.model().relationships.iter()
        .find(|r| r.technology == Some("REST/HTTPS".to_string()));
    assert!(rel_with_api.is_some(), "Should have relationship with REST/HTTPS technology");

    // These are all root-level elements; graph reachability is not containment.
    assert_eq!(workspace.model().relationships.len(), 4);

    // Test docs and adrs properties
    assert_eq!(
        workspace.get_property("structurizr.docs"),
        Some(&"documentation".to_string())
    );
    assert_eq!(
        workspace.get_property("structurizr.adrs"),
        Some(&"architecture-decisions".to_string())
    );

    // Test workspace metadata
    assert_eq!(workspace.name, "E-Commerce Platform");
    assert_eq!(
        workspace.description,
        Some("An example with all directives".to_string())
    );

    // Test views
    assert_eq!(workspace.views().system_context_views.len(), 1);
}

#[test]
fn test_const_directive_multiple_substitutions() {
    let dsl = r##"
workspace {
    !const ENV "Production"
    !const REGION "US-East"

    model {
        system = softwareSystem "System" "Deployed in ${ENV} environment in ${REGION} region"
    }
}
"##;

    let workspace = parse(dsl).unwrap();
    assert_eq!(
        workspace.model().software_systems[0].properties.description,
        Some("Deployed in Production environment in US-East region".to_string())
    );
}

#[test]
fn test_implied_relationships_complex_chain() {
    let dsl = r##"
workspace {
    !impliedRelationships true

    model {
        a = person "A"
        b = softwareSystem "B"
        c = softwareSystem "C"
        d = softwareSystem "D"

        a -> b "Uses"
        b -> c "Calls"
        c -> d "Depends on"
    }
}
"##;

    let workspace = parse(dsl).unwrap();

    // No containment exists in this model, so no ancestor links can be implied.
    assert_eq!(workspace.model().relationships.len(), 3);
}

#[test]
fn graph_chains_do_not_create_implied_relationships() {
    let dsl = r##"
workspace {
    model {
        a = person "A"
        b = softwareSystem "B"
        c = softwareSystem "C"

        a -> b "Uses"
        b -> c "Calls"
    }
}
"##;

    let workspace = parse(dsl).unwrap();
    // Should only have the 2 explicit relationships
    assert_eq!(workspace.model().relationships.len(), 2);
}

#[test]
fn test_const_in_view_titles() {
    let dsl = r##"
workspace {
    !const SYSTEM_NAME "Payment System"

    model {
        sys = softwareSystem "${SYSTEM_NAME}"
    }

    views {
        systemContext sys "${SYSTEM_NAME}-Context" "Context for ${SYSTEM_NAME}" {
            include *
        }
    }
}
"##;

    let workspace = parse(dsl).unwrap();
    let view = &workspace.views().system_context_views[0];
    assert_eq!(view.properties.key, "Payment System-Context");
    assert_eq!(view.properties.title, Some("Context for Payment System".to_string()));
}

/// Execution-dependent workspaces must not become apparently complete previews.
#[test]
fn script_directives_require_a_self_contained_workspace() {
    assert!(parse("workspace { !script \"script.lua\" }").is_err());
}
