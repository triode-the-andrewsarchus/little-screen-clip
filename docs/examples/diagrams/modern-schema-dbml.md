# Native DBML schema review

Partials retain the winning column order. Records stay sample text; the open
circle on the author relationship marks its explicit optional referenced end.

```dbml
TablePartial identity {
  id int [pk]
  name varchar(50)
}

TablePartial audit {
  created_at timestamp
}

Enum State {
  active
  inactive
}

Table users {
  ~identity
  ~audit
  name varchar(100) [not null]
  state State

  records [example] {
    1, '2024-01-15', 'Alice, O\'Brien', State.active
    2, `now()`, 'Bob', 'inactive'
  }
}

Table posts {
  id int [pk]
  author_id int [ref: >? users.id]
  title varchar
}

records posts(id, author_id, title) [example] {
  10, 1, 'A post with an author'
  11, , 'A post without an author'
}
```
