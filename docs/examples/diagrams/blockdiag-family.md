# Blockdiag Family previews

Local source and rendered views of the same diagrams.

## pipeline.blockdiag

```blockdiag
blockdiag {
  default_fontsize = 16;
  node_width = 140;
  node_height = 60;
  source [label = "Source", color = "#d9e8fb"];
  parse [label = "Parse", shape = roundedbox];
  preview [label = "Preview", color = "#d9f0df"];
  source -> parse -> preview;
  group validation {
    label = "Validation";
    color = "#eeeeee";
    parse;
    rejected [label = "Error", shape = diamond, color = "#f7ddd8"];
  }
  parse -> rejected [label = "invalid", style = dashed];
}
```

## request.seqdiag

```seqdiag
seqdiag {
  default_fontsize = 14;
  browser [color = "#d9e8fb"];
  browser -> service [label = "GET /preview"];
  service -> cache [label = "Find source"];
  service <-- cache [label = "Cached diagram"];
  browser <-- service [label = "SVG preview"];
}
```

## review.actdiag

```actdiag
actdiag {
  draft -> review -> publish;
  draft [label = "Draft"];
  review [label = "Review", color = "#d9e8fb"];
  publish [label = "Publish", color = "#d9f0df"];
  lane author {
    label = "Author";
    draft;
    publish;
  }
  lane editor {
    label = "Editor";
    review;
  }
}
```

## service.nwdiag

```nwdiag
nwdiag {
  network public {
    address = "192.0.2.0/24";
    gateway [address = "192.0.2.1"];
    web [address = "192.0.2.20", color = "#d9e8fb"];
  }
  network internal {
    address = "10.0.0.0/24";
    web [address = "10.0.0.20"];
    database [address = "10.0.0.30", color = "#d9f0df"];
  }
}
```

## header.packetdiag

```packetdiag
packetdiag {
  colwidth = 32;
  node_height = 64;
  0-3: Version [color = "#d9e8fb"];
  4-7: Flags;
  8-15: Length [color = "#d9f0df"];
  16-31: Identifier;
  32-63: Payload;
}
```

## server.rackdiag

```rackdiag
rackdiag {
  rack {
    12U;
    description = "Preview rack";
    1: Switch [color = "#d9e8fb"];
    3: Render [2U, color = "#d9f0df"];
    6: Storage [3U];
    10: Backup [2U, color = "#d9f0df"];
  }
}
```
