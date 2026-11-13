# Database schema

```dbml
// Tables and their real column relationships.
Table users {
  id int [pk, increment]
  name varchar(80) [not null]
}
Table posts {
  id int [pk]
  user_id int [ref: > users.id]
  title varchar [default: 'draft']
}
```
