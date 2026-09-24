# Unique fields

A `unique` block, after the schema, lists fields that no two nodes of a type may
share. The engine refuses any write that would break it, so a name or an email
address can be trusted to find one node.

```zql
schema {
  type Player {
    name: String
    email: String
    salary: Int
  }
}

unique {
  Player { name email }
}
```

Each field listed is unique on its own: here no two players share a name, and
no two share an email. It is not a rule about the pair.

```zql
mutation {
  Player(name: "Alice" && email: "alice@example.test" && salary: 12500000)
}
```

```zql
mutation {
  Player(name: "Bob" && email: "bob@example.test" && salary: 950000)
}
```

## Creating a duplicate

A second Alice is refused, and nothing in that mutation is written.

```zql error
mutation {
  Player(name: "Alice" && email: "other@example.test" && salary: 1)
}
```

```text
execution error: error: unique Player { name } is already used
  query:2:3
    Player(name: "Alice" && email: "other@example.test" && salary: 1)
    ^^^^^^
  help: another Player already has this name
```

## Changing a field to a used value

`set` is checked the same way. Setting a field to the value it already has is
fine; it only has to differ from every other node.

```zql error
mutation {
  Player(name: "Bob") set email: "alice@example.test"
}
```

```text
execution error: error: unique Player { email } is already used
  query:2:27
    Player(name: "Bob") set email: "alice@example.test"
                            ^^^^^
  help: another Player already has this email
```

```zql
mutation {
  Player(name: "Bob") set name: "Bob", salary: 1000000
}
```

```zql
query {
  Player(name: "Bob") { email salary }
}
```

```json
{ "email": "bob@example.test", "salary": 1000000 }
```

## What can be unique

Only fields that hold a value can be unique. A relationship cannot:

```zql error
schema {
  type Player {
    name: String
    teammates -> Player[]
  }
}

unique {
  Player { teammates }
}
```

```text
execution error: error: Player.teammates is a relationship
  schema:9:12
    Player { teammates }
             ^^^^^^^^^
  help: unique applies to a field, such as `name`
```

A unique field is also indexed for lookups, so `Player(name: "Alice")` finds
Alice without reading every player; see [indexes](index.md).

## Next

- [Schema](schema.md): where `unique` goes.
- [Mutations](mutation.md): all-or-nothing writes.
