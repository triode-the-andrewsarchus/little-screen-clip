# Native diagrams

```plantuml
@startuml
skinparam ParticipantBackgroundColor #b8d8ec
participant Browser
participant Renderer
Browser -> Renderer: Preview source
activate Renderer
Renderer --> Browser: Native artwork
 deactivate Renderer
@enduml
```

```c4plantuml
@startuml
!include <C4/C4_Container>
Person(reader, "Reader", "Explores local files")
System(browser, "File manager", "Interactive previews")
Rel(reader, browser, "Browses")
@enduml
```
