# People and places

```erd
# A compact Erd schema with primary/foreign keys and exact cardinalities.
title {label: "People and places", size: "18"}
entity {bgcolor: "#d0e0d0"}
[Person]
*id {label: "integer"}
name {label: "text"}
+birth_place_id
[Place] {bgcolor: "#ececfc"}
*id
city
Person *--1 Place {label: "born in"}
```
