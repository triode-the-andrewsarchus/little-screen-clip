# Architecture previews

The source below stays editable.

## Container view

```structurizr
workspace "Preview pipeline" "Native architecture views" {
    model {
        reader = person "Reader" "Explores files"
        app = softwareSystem "File manager" {
            browser = container "Browser" "Selects and inspects files" "Rust"
            previews = container "Preview worker" "Renders bounded previews" "Rust"
        }
        reader -> browser "Browses"
        browser -> previews "Requests a preview" "IPC"
    }
    views {
        container app "Pipeline" {
            title "Preview pipeline"
            include *
            autoLayout lr
        }
        styles {
            element "Container" { background "#295f98" color "#ffffff" }
            element "Person" { background "#704099" color "#ffffff" }
        }
    }
}
```

## Request and reply

```structurizr
workspace "Request flow" {
    model {
        user = person "Reader"
        app = softwareSystem "File manager"
        user -> app "Browses" "Desktop"
    }
    views {
        dynamic * "Requests" {
            title "Request and reply"
            user -> app "Open a file"
            app -> user "Show preview"
        }
        styles { element "Software System" { background "#295f98" color "#ffffff" } }
    }
}
```

## Custom work stages

```structurizr
workspace "Work stages" {
    model {
        source = element "Source" "Input" "An authored file"
        preview = element "Preview" "Output" "A rendered view"
        source -> preview "Render"
    }
    views {
        custom "Stages" "Work stages" { include * autoLayout lr }
        styles { element "Element" { background "#295f98" color "#ffffff" } }
    }
}
```

Deployment and image views currently report an explicit unsupported-preview error.
