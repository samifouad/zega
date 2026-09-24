# Errors

When ZQL cannot run, the engine says what is wrong, points at the exact place in
your code, and usually says how to fix it. Nothing is written by a statement
that fails.

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
```

```zql
mutation {
  Team(name: "Oilers" && city: "Edmonton") {
    players <- Player(name: "Alice" && salary: 12500000)
    players <- Player(name: "Bob" && salary: 950000)
  }
}
```

## Reading an error

```zql error
query {
  Player { name salry }
}
```

```text
execution error: error: Player has no field salry
  query:2:17
    Player { name salry }
                  ^^^^^
  help: did you mean `salary`?
```

- The first line is the problem. `execution error:` is how the Rust API labels
  errors from the engine.
- `query:2:17` is where: the source, then the line and column. `query` is the
  statement you ran; `schema` is the schema, or a whole `.zql` file run at once.
- The source line is repeated with `^` under the exact text at fault.
- `help:` suggests a fix, here the field name you probably meant.

## Common errors

### A name that is not in the schema

A misspelt type or field, or one the schema does not have, is an error rather
than an empty result. Check the spelling against the schema; `help` lists what
there is.

```zql error
query {
  Players { name }
}
```

```text
execution error: error: unknown type Players
  query:2:3
    Players { name }
    ^^^^^^^
  help: did you mean `Player`?
```

### More than one match where one is expected

A filter of only equality tests expects to name one node (see
[say one, get one](query.md#say-one-get-one)), and so do the nodes in `set` and
`link`. When two match, ZQL fails rather than pick one. Make the filter
narrower, use a comparison to get a list, or make the field
[unique](unique.md).

```zql
mutation {
  Player(name: "Alice" && salary: 700000)
}
```

```zql error
query {
  Player(name: "Alice") { salary }
}
```

```text
execution error: error: Player matched 2 rows
  query:2:3
    Player(name: "Alice") { salary }
    ^^^^^^
  help: an equality filter has to match one row
```

### No match for set or link

`set` and `link` change nodes that already exist, so a filter that matches none
is an error. Create the node first, or fix the filter.

```zql error
mutation {
  Player(name: "Zed") set salary: 1
}
```

```text
execution error: error: no Player matched
  query:2:3
    Player(name: "Zed") set salary: 1
    ^^^^^^
  help: `link` and `set` need exactly one matching row
```

### A comparison where a value is expected

In a mutation, the parentheses on a new node set its fields, so they only take
`field: value` joined with `&&`. To change nodes that match a test, use `set`
with a filter that names one node.

```zql error
mutation {
  Player(salary > 1000000)
}
```

```text
execution error: error: creating a Player only accepts field: value
  query:2:10
    Player(salary > 1000000)
           ^^^^^^
  help: write `name: "value"`, and join fields with `&&`
```

### A comma between conditions

Conditions and fields are joined with `&&`. A comma separates the writes in a
`set`, and nothing else.

```zql error
mutation {
  Player(name: "Cara", salary: 1)
}
```

```text
execution error: error: and is `&&`
  query:2:22
    Player(name: "Cara", salary: 1)
                       ^
  help: a comma separates writes in `set`
```

### A missing brace

An unclosed block is reported where the code ended, since that is where the
brace was due.

```zql error
query {
  Team { name
}
```

```text
execution error: error: expected }
  query:4:1
  
  ^
```

### Breaking a unique rule

Writing a value that another node already has in a [unique](unique.md) field
is refused, and the whole mutation is undone.

## Next

- [Queries](query.md) and [mutations](mutation.md), for what each statement
  expects.
- [Conditions](conditions.md), for everything a filter accepts.
