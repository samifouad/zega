# Conditions

The parentheses after a type in a [query](query.md) are its filter: a condition
each node has to meet to be in the result.

```zql
schema {
  type Player {
    name: String
    position: String
    salary: Int
    rookie: Bool
    bio?: String
  }
}
```

```zql
mutation {
  Player(
    name: "Alice" &&
    position: "C" &&
    salary: 12500000 &&
    rookie: false &&
    bio: "Captain since 2021"
  )
}
```

```zql
mutation {
  Player(name: "Bob" && position: "D" && salary: 950000 && rookie: true)
}
```

```zql
mutation {
  Player(
    name: "Cara" &&
    position: "LW" &&
    salary: 4000000 &&
    rookie: false &&
    bio: "Drafted in 2019"
  )
}
```

## Equality and comparison

`=` tests that a field equals a value; `:` means the same in a filter. `!=` (or
`<>`) is not-equal, and `>`, `<`, `>=` and `<=` compare numbers and text.

```zql
query {
  Player(salary >= 4000000) { name salary }
}
```

```json
[
  { "name": "Alice", "salary": 12500000 },
  { "name": "Cara", "salary": 4000000 }
]
```

```zql
query {
  Player(position != "C") { name }
}
```

```json
[
  { "name": "Bob" },
  { "name": "Cara" }
]
```

A filter made only of equality tests names one node, so it returns an object
instead of a list; see [say one, get one](query.md#say-one-get-one).

```zql
query {
  Player(rookie = true) { name }
}
```

```json
{ "name": "Bob" }
```

## && and ||

`&&` means both, `||` means either. `&&` binds tighter than `||`, the way `×`
binds tighter than `+`, so `a || b && c` means `a || (b && c)`. Use parentheses
to group differently.

```zql
query {
  Player((position = "C" || position = "D") && salary < 5000000) { name }
}
```

```json
[
  { "name": "Bob" }
]
```

Without the parentheses, the same tests mean something else: every centre, or
a defender under 5 million.

```zql
query {
  Player(position = "C" || position = "D" && salary < 5000000) { name }
}
```

```json
[
  { "name": "Alice" },
  { "name": "Bob" }
]
```

## Text

`findWith` finds text anywhere in a field, and `startsWith` and `endsWith`
match its start and end. All three are case-sensitive.

```zql
query {
  Player(bio findWith "20") { name bio }
}
```

```json
[
  { "bio": "Captain since 2021", "name": "Alice" },
  { "bio": "Drafted in 2019", "name": "Cara" }
]
```

```zql
query {
  Player(name startsWith "C" || name endsWith "b") { name }
}
```

```json
[
  { "name": "Bob" },
  { "name": "Cara" }
]
```

A node without an optional field does not match a text test on it: Bob has no
`bio`, so he is not in the first list. It does match `!=`, because a missing
value is not equal to anything.

```zql
query {
  Player(bio != "Drafted in 2019") { name }
}
```

```json
[
  { "name": "Alice" },
  { "name": "Bob" }
]
```

Regular expressions and `findWithout` (text a node must not contain) search
whole nodes rather than one field. They are part of
[discovery with `then`](then.md), not of a filter.

## Filters in mutations

In a [mutation](mutation.md), parentheses on a new node set its fields, so only
`field: value` joined with `&&` is allowed there. In `set` and `link`, the
parentheses are a filter again, and have to match exactly one node.

## Next

- [Relationships](relationships.md): filters on related nodes.
- [Indexes](index.md) make equality, range and text tests fast.
