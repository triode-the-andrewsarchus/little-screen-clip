# Nested deployment preview

The same source is available as `deployment.structurizr`. The two API placements
keep separate identities and share static labels/styles. Host boundaries show
real containment; SQL, gateway and self relationships remain visible.

```structurizr
workspace "Placement review" {
    model {
        shop = softwareSystem "Shop" {
            api = container "API" "Accepts requests" "Rust"
            db = container "Database" "Stores orders" "PostgreSQL"
        }
        api -> db "Queries" "SQL"
        prod = deploymentEnvironment "Production" {
            region = deploymentNode "Cloud region" "Selected production placement" "Region" {
                gateway = infrastructureNode "Gateway" "Routes requests" "TLS"
                hostA = deploymentNode "Host A" "Application host" "Linux" { apiA = containerInstance api }
                hostB = deploymentNode "Host B" "Application and storage host" "Linux" {
                    apiB = containerInstance api
                    database = containerInstance db
                }
                gateway -> apiA "Routes" "HTTPS"
                apiA -> apiA "Health" "Local"
            }
        }
    }
    views {
        deployment shop prod "Production" { include * autoLayout lr }
        styles {
            element "Deployment Node" { background "#edf1f5" color "#26354a" stroke "#61758d" }
            element "Container" { background "#295f98" color "#ffffff" }
            element "Infrastructure Node" { background "#f3dfc0" color "#26354a" }
            relationship "Relationship" { color "#a91f72" routing Orthogonal }
        }
    }
}
```

External image views remain unsupported. File resources, themes, includes, health
checks and scripts are not loaded or executed by this preview.
