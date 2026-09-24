# Names in ZQL (APS 6)

[APS 6](https://github.com/zegadb/aps/issues/6) gives language-defined names an
`@` prefix. User node fields keep their bare names; user edge fields keep `&`.
No property name is reserved.

| Language-owned name | Context |
| --- | --- |
| `@id` | Stored node identity in filters and projections |
| `@hops` | Measured traversal depth; an unweighted path bound |
| `@cost` | Path cost bound |
| `@score` | Similarity score in an `@near` selection |
| `@point`, `@vector` | Point and vector literals |
| `@distance`, `@similarity`, `@within_box` | Spatial/vector calculations and predicates |
| `@near` | Nearest-vector selection |

```zql
schema {
  type Stop {
    name: String
    hops: Int
    cost: Int
    road -> Stop[] {
      hops: Int
      cost: Int
      shape: String
    }
  }

  display {
    graph : Default
    table
  }
}

mutation {
  Stop(name: "A" && hops: 11 && cost: 12) {
    road -> Stop(name: "B" && hops: 21 && cost: 22) {
      &hops: 7
      &cost: 8
      &shape: "circle"
    }
  }
}

query {
  Stop(name: "A") {
    road *path(@cost <= 8) by &cost -> Stop(name: "B") {
      name
      &hops
      &cost
      &shape
      depth: @hops
      node: @id
    }
  }
}
```

Here `&hops` returns the stored edge value `7`, while `depth: @hops` returns
measured depth `1` at B. Bare `hops` would return B's stored node value `21`.
Aliases let stored and built-in values appear in the same result. Default JSON
result keys remain `id`, `hops`, `score`, `distance`, and `similarity`.

String filters are `startsWith`, `endsWith`, and `findWith` (substring matching).
They are infix operators, so they have no `@`. They remain case sensitive and
use the same text indexes. `findWith` does not conflict with another ZQL
operator or function. These names are also available as user property names.

Block and positional keywords retain their spelling: `schema`, `type`, `query`,
`mutation`, `display`, `unique`, `index`, `graph`, `table`, `map`, `globe`, `timeline`,
`vector2d`, `vector3d`, `range`, `text`, `set`, `link`, `from`, `order`, `by`,
`limit`, `toward`, `path`, `json`, and `csv`. Enum/type/literal values such as
`exact`, `Default`, `cosine`, `dot`, `l2`, `km`, `String`, `true`, and `null`
likewise have no prefix. `$Name` remains an import column binding.

Display options are language names and take `@`: the per-type `@shape`,
`@size` and `@image` ([APS 8](https://github.com/zegadb/aps/issues/8)) and the
globe's `@zoom`, `@tilt` and `@center`
([APS 9](https://github.com/zegadb/aps/issues/9)), whose value is a built-in
`@point(...)`. See [schema display](../browser/docs/display.md).

The removed spellings produce source-spanned errors naming the replacement.
When `hops`, `id`, or `score` is declared as a user field, it reads that field;
when the old implicit spelling has no matching declaration, the checker gives
migration help. Cypher uses its own parser and retains its own syntax.
