# Queries

A `query` block reads from the graph. You write the shape you want back, and the
result has exactly that shape: the fields you name, nested the way you nest them.

The examples on this page share this schema and data.

```zql
schema {
  type Team {
    name: String
    city: String
    at: Point
    players: MEMBER <- Player[]
    captain -> Player
  }

  type Player {
    name: String
    position: String
    salary: Int
    playsFor: MEMBER -> Team
  }
}
```

```zql
mutation {
  Team(name: "Oilers" && city: "Edmonton" && at: @point(53.5, -113.5)) {
    players <- Player(name: "Alice" && position: "C" && salary: 12500000)
    players <- Player(name: "Bob" && position: "D" && salary: 950000)
    players <- Player(name: "Cara" && position: "LW" && salary: 4000000)
  }
}
```

```zql
mutation {
  Team(name: "Flames" && city: "Calgary" && at: @point(51.0, -114.1))
}
```

## Selecting fields

Name a type, then the fields you want in braces. Every node of that type comes
back, with only those fields.

```zql
query {
  Player { name position }
}
```

```json
[
  { "name": "Alice", "position": "C" },
  { "name": "Bob", "position": "D" },
  { "name": "Cara", "position": "LW" }
]
```

A field that is not in the schema is an error, not a missing value; see
[errors](errors.md). Keys in the result are in alphabetical order.

## Following relationships

A relationship in the selection, with its own braces, nests the related nodes
inside each result. A relationship to many (`Player[]` in the schema) gives a
list, empty when there are none; a relationship to one gives an object, or
`null`.

```zql
query {
  Team {
    name
    captain -> Player { name }
    players <- Player { name }
  }
}
```

```json
[
  {
    "captain": null,
    "name": "Oilers",
    "players": [
      { "name": "Alice" },
      { "name": "Bob" },
      { "name": "Cara" }
    ]
  },
  {
    "captain": null,
    "name": "Flames",
    "players": []
  }
]
```

[Relationships](relationships.md) covers arrows in both directions and walking
several hops.

## Say one, get one

When the filter is only equality tests joined by `&&`, you are naming one node,
so the result is one object instead of a list.

```zql
query {
  Player(name: "Alice") {
    name
    playsFor -> Team { name city }
  }
}
```

```json
{ "name": "Alice", "playsFor": { "city": "Edmonton", "name": "Oilers" } }
```

If nothing matches, the result is `null`.

```zql
query {
  Player(name: "Zed") { name }
}
```

```json
null
```

If more than one node matches, the query fails instead of guessing; see
[errors](errors.md). Any other filter, such as a comparison, gives a list, which
may be empty. [Conditions](conditions.md) lists every test a filter can use.

```zql
query {
  Player(salary > 1000000) { name }
}
```

```json
[
  { "name": "Alice" },
  { "name": "Cara" }
]
```

## limit

`limit n` after the type keeps the first `n` results. A query with `limit`
always returns a list.

```zql
query {
  Player limit 2 { name }
}
```

```json
[
  { "name": "Alice" },
  { "name": "Bob" }
]
```

## Order

Results come back in the order the nodes were created. The one ordering ZQL has
today is by distance from a point, `order by @distance(field, @point(lat, lon))`,
nearest first; see [locations](location.md). Ordering by other fields is not in
the language yet ([zegadb/zega#73](https://github.com/zegadb/zega/issues/73)).

```zql
query {
  Team order by @distance(at, @point(51.0, -114.0)) limit 1 { name }
}
```

```json
[
  { "name": "Flames" }
]
```

## Node ids

`@id` selects the id the engine gave a node, and `@id = n` in a filter finds a
node by it.

```zql
query {
  Player(name: "Bob") { @id name }
}
```

```json
{ "id": 3, "name": "Bob" }
```

## Next

- [Conditions](conditions.md): everything that can go in a filter.
- [Relationships](relationships.md): walking the graph.
