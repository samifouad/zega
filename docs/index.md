# Indexes

Name an index only when a query needs one. A read without an index tests
every row of its type; with one, it tests only the rows the index returns.
The answer is the same either way.

```zql
schema {
  type Player {
    name: String
    salary: Int
    rating?: Float
  }
}

unique {
  Player { name }
}

index {
  range Player { salary rating }
  text Player { name }
}
```

- `range` speeds `=`, `>`, `<`, `>=` and `<=`, and several of them on one field
  (`salary > 100 && salary < 200` is one ordered scan). It applies to `Int`,
  `Float` and `String` fields.
- `text` speeds `findWith`, `startsWith` and `endsWith`. It applies to
  `String` fields. A range index does not help these.
- Each field in the braces gets its own index. One field may have both kinds.
- A `unique` field already has a range index. Naming it under `range` is an
  error rather than a second index.
- `&&` narrows by every indexed condition; `||` uses indexes only when both
  sides have one. `!=` never uses an index.
- A read over several types (`(Player | Coach)`) uses an index only when every
  type has it.

`index` and `unique` follow the types, in either order, once each.

## Checks

The block is type-checked with the schema, at the name's span:

```
error: text index needs a String field; Player.salary is Int
  schema:6:17
    text Player { salary }
                  ^^^^^^
  help: `text` speeds findWith, startsWith and endsWith on a String
```

Also errors: an unknown type or field, a relationship, `range` on a `Bool`,
`Point` or `Vector` field (`Point` and `Vector` fields are indexed
automatically), the same field twice under one kind, and a second `index`
block.

## How it works

A declared index is kept in step with every insert, `set` and delete. Indexes
are built from the data, like the label and equality indexes: after a restart
or a snapshot restore, the first statement whose schema declares an index
builds it from the replayed rows. A statement whose schema no longer declares
an index drops it.

- `range` is an ordered map from value to rows per type and field. Numbers and
  strings are kept apart, as a comparison between them never matches.
- `text` is a trigram index. Each value is padded with a start and an end
  marker, so the trigrams of a needle, of start+needle and of needle+end answer
  `findWith`, `startsWith` and `endsWith` from one structure that is updated
  in place on every write. A needle too short for a trigram (under three
  characters for `findWith`, under two for the others) is answered from every
  row with a string in that field.

An index returns candidates: every row that can match, and sometimes a few
that cannot (`>` is read as `>=`). The same filter that a scan uses then
tests each candidate, so an index never changes a result.

`Zega::rows_examined()` (and `rows_examined()` in the WASM build) counts the
rows a filter has tested, which is how the tests show an index was used.
