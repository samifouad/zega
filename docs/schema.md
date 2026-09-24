# Schema

A ZQL document starts with a `schema`. It names the types of node in the graph,
the fields each one has, and the relationships between them. Every mutation and
query is checked against it, so a typo in a field name is an error, not an empty
result.

```zql
schema {
  type Team {
    name: String
    city: String
    founded: Int
  }

  type Player {
    name: String
    salary: Int
    longestShot?: Float<m>
    bio?: String
    homepage?: String<url>
    playsFor -> Team
  }
}
```

## Types and fields

Each `type` is one kind of node. A field is a name, a colon and a value type:

| Type | Holds |
|---|---|
| `String` | Text. |
| `Int` | A whole number. |
| `Float` | A number with a fraction. |
| `Bool` | `true` or `false`. |
| `Point` | A latitude and longitude. See [locations](location.md). |
| `Vector<N>` | N numbers, for similarity search. See [vectors](vector.md). |

## Required and optional fields

A field whose name ends in `?` is optional: a node may leave it out, and a
query reads it as `null`.

```zql
mutation {
  Team(name: "Oilers" && city: "Edmonton" && founded: 1972)
}
```

```zql
mutation {
  Player(name: "Alice" && salary: 12500000) {
    playsFor -> Team(name: "Oilers")
  }
}
```

```zql
query {
  Player(name: "Alice") { name bio }
}
```

```json
{ "bio": null, "name": "Alice" }
```

A field without `?` is required. Today the engine does not yet reject a node
created without one; it reads as `null` too
([zegadb/zega#72](https://github.com/zegadb/zega/issues/72)). Required
properties on a relationship are enforced; see
[relationships](relationships.md).

## Units

A distance field can carry its unit in angle brackets: `Float<m>`, `Float<km>`
or `Float<mi>`. The unit is part of the type, so metres are never added to
kilometres by mistake; [paths](path.md) use units to add up the length of a
route. `String<url>` marks a string that holds a web address, so the explorer
can show it as a link.

## Relationships

A field with an arrow is a relationship to another type. `playsFor -> Team`
points at one team; `players -> Player[]`, with brackets, points at any number
of players. See [relationships](relationships.md) for arrows in both
directions, named relationship types and properties on the relationship itself.

## Unique fields

A `unique` block after the schema says which fields no two nodes of a type may
share. See [unique fields](unique.md).

## Next

- [Mutations](mutation.md) create and change nodes.
- [Queries](query.md) read them back.
