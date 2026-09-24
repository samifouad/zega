# Mutations

A `mutation` block writes to the graph: it creates nodes and relationships,
changes fields with `set` and connects existing nodes with `link`. Each block is
all or nothing.

The examples on this page share one schema. `unique` says no two teams or
players share a name; see [unique fields](unique.md).

```zql
schema {
  type Team {
    name: String
    city: String
    players: MEMBER <- Player[]
  }

  type Player {
    name: String
    salary: Int
    playsFor: MEMBER -> Team
  }
}

unique {
  Team { name }
  Player { name }
}
```

## Creating a node

A type followed by its fields in parentheses creates one node. Fields are set
with `:` and joined with `&&`.

```zql
mutation {
  Team(name: "Oilers" && city: "Edmonton")
}
```

A mutation returns what it selects, in the same shape as a
[query](query.md). With nothing selected, that is an empty object.

```json
{}
```

## Creating related nodes together

A block after the node creates related nodes with it, through the
relationships in the schema. Each nested node is new, and the relationship
between them is created too.

```zql
mutation {
  Team(name: "Flames" && city: "Calgary") {
    players <- Player(name: "Alice" && salary: 12500000) { name }
    players <- Player(name: "Bob" && salary: 950000) { name }
  }
}
```

```json
{
  "players": [
    { "name": "Alice" },
    { "name": "Bob" }
  ]
}
```

Every node a mutation names in this way is created, even if one like it exists:
without `unique`, running the first mutation twice would make two Oilers.

## Changing fields with set

`set` changes fields on one existing node. The filter in parentheses has to
match exactly one node; separate several fields with commas.

```zql
mutation {
  Player(name: "Bob") set salary: 1000000, name: "Bobby"
}
```

```zql
query {
  Player(name: "Bobby") { name salary }
}
```

```json
{ "name": "Bobby", "salary": 1000000 }
```

## Connecting existing nodes with link

`link` connects two nodes that already exist. The node at the top has to match
exactly one existing node, and so does the one after `link`. To add a new player
to an existing team, create the player, then link it.

```zql
mutation {
  Player(name: "Dana" && salary: 800000)
}
```

```zql
mutation {
  Player(name: "Dana") {
    playsFor -> link Team(name: "Oilers")
  }
}
```

```zql
query {
  Team(name: "Oilers") {
    players <- Player { name }
  }
}
```

```json
{
  "players": [
    { "name": "Dana" }
  ]
}
```

## All or nothing

A mutation block is applied completely or not at all. Here the second player
breaks the unique rule on names, so the whole block fails: the Canucks and
Evan are not created either.

```zql error
mutation {
  Team(name: "Canucks" && city: "Vancouver") {
    players <- Player(name: "Evan" && salary: 700000)
    players <- Player(name: "Alice" && salary: 1)
  }
}
```

```text
execution error: error: unique Player { name } is already used
  query:4:16
      players <- Player(name: "Alice" && salary: 1)
                 ^^^^^^
  help: another Player already has this name
```

```zql
query {
  Team { name }
}
```

```json
[
  { "name": "Oilers" },
  { "name": "Flames" }
]
```

## Deleting

ZQL has no statement that deletes a node or a relationship yet
([zegadb/zega#73](https://github.com/zegadb/zega/issues/73)). From Rust,
`Zega::delete_node(id)` removes a node and its relationships.

## Loading JSON and CSV

A mutation can also create one node per row of a JSON or CSV file, with
`mutation json [...]` and `mutation csv [...]`. A load is all or nothing in the
same way. See [loading data](data-loading.md).

## Next

- [Queries](query.md) read what you wrote.
- [Errors](errors.md) explains what a failed mutation reports.
