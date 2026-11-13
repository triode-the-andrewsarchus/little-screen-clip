# Message layout

```bytefield
;; Bytefield preview: a small message header with variable payload.
(def boxes-per-row 8)
(defattrs :header {:fill "#b8d8ec"})
(draw-column-headers)
(draw-box "Type" [:header {:span 2}])
(draw-box "Flags" [:header {:span 2}])
(draw-box "Length" [:header {:span 4}])
(draw-related-boxes [0x01 0x00 0x00 0x08 0x41 0x42 0x43 0x44])
(draw-gap "Payload")
(draw-bottom)
```
