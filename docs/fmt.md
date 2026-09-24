# Formatting ZQL and JSON with zega fmt

`zega fmt` gives every ZQL and JSON file one layout. There are no options and
no style settings: two files that mean the same thing look the same. The rules
were locked in [APS 12](https://github.com/zegadb/aps/issues/12), and this page
shows each of them on real input.

The formatter reads the file with the same parser the engine uses, then prints
it again. If the input does not parse, for example a query you are halfway
through typing, `zega fmt` returns it byte for byte unchanged. It never guesses.

## Running it

Format files or whole directories in place. Directories are searched
recursively for `*.zql` and `*.json` files; hidden directories, `node_modules`,
`target` and `dist` are skipped.

```sh
zega fmt schema.zql queries/
```

Check without writing, for CI. Every file that would change is listed.

```sh
zega fmt --check .
```

| Exit code | Meaning |
|---|---|
| `0` | Every file is already formatted (or, without `--check`, the files were formatted). |
| `1` | With `--check`, at least one file would be reformatted. Nothing else exits `1`. |
| `2` | `zega fmt` could not do its job: the arguments are wrong (for example no paths and no `--stdin`), or a file or directory could not be read or written. The error, starting `zega fmt:`, names the path. |

Input that does not parse is left unchanged, so `--check` passes on it. Use the
engine or the explorer to find syntax errors; `zega fmt` only lays out valid
code.

Format standard input to standard output, for editors. The language is ZQL
unless you say otherwise.

```sh
zega fmt --stdin < query.zql
zega fmt --stdin --lang json < data.json
```

The explorer uses the same formatter, compiled to WASM. Press ⌘S / Ctrl-S or
**Format** to format and save the active schema or query pane. Formatting is
undoable and keeps the cursor on its line. The explorer's JSON import preview
and result views are printed by the same JSON formatter.

## Examples

Every example below is real output. A test in zegadb/zega formats each
**before** block and requires the **after** block that follows it, byte for
byte, so this page cannot drift from the formatter.

### A cramped query

Top-level blocks such as `query` always open up, one item per line. A selection
of one or two plain fields stays on one line when it fits in 80 columns, like
`{ name salary }` here; three or more fields, or any nested block, get a line
each. That is why `Team` opens up: it contains the nested `players` block.
Colons get one space after them, arrows and comparison operators one space on
each side.

```zql before
query{Team(country:"CA"){name players->Player(salary>10000000){name salary}}}
```

```zql after
query {
  Team(country: "CA") {
    name
    players -> Player(salary > 10000000) { name salary }
  }
}
```

### A schema

A type with one field stays on one line, like `Country`; a type with two or more
fields puts one field on each line. One blank line separates the types, and the
blocks inside `schema`.

Display views follow the selection rule: a view of one or two types stays on
one line, and `graph { … } : Default` keeps its default marker with it. A view of
three or more types puts one type on each line.

```zql before
schema{type Country{name:String}type Team{name:String country->Country}type Player{name:String salary:Int at?:Point playsFor->Team}display{graph{Team Player}:Default map{Player} table{Country Team Player}}}
```

```zql after
schema {
  type Country { name: String }

  type Team {
    name: String
    country -> Country
  }

  type Player {
    name: String
    salary: Int
    at?: Point
    playsFor -> Team
  }

  display {
    graph { Team Player } : Default
    map { Player }
    table {
      Country
      Team
      Player
    }
  }
}
```

### Mutations

Every top-level block is separated from the next by one blank line. A filter or
argument list stays on one line while it fits in 80 columns, so
`(name: "Oilers" && country: "CA")` is kept together. Load sources keep their
list of files next to the keyword.

```zql before
mutation{Team(name:"Oilers"&&country:"CA"){players->Player(name:"Alice"&&salary:12500000){name}}}mutation csv ["./players.csv"]{Player(name:$Name&&salary:$Salary){name salary}}
```

```zql after
mutation {
  Team(name: "Oilers" && country: "CA") {
    players -> Player(name: "Alice" && salary: 12500000) { name }
  }
}

mutation csv ["./players.csv"] {
  Player(name: $Name && salary: $Salary) { name salary }
}
```

### Long conditions with && and ||

When a filter would pass 80 columns, it opens up: `(` ends the first line, each
operand of the `&&` chain gets its own line with the operator at the end, and
`)` closes on its own line. Nothing is ever half-wrapped. Parentheses you wrote
are kept, so `(position = "C" || position = "LW")` stays one operand.

```zql before
query{Player((position="C"||position="LW")&&salary>=5000000&&salary<12000000&&name startsWith "A"){name position salary}}
```

```zql after
query {
  Player(
    (position = "C" || position = "LW") &&
    salary >= 5000000 &&
    salary < 12000000 &&
    name startsWith "A"
  ) {
    name
    position
    salary
  }
}
```

### Comments stay where you put them

ZQL has one comment style, `//`, on its own line or at the end of a line with one
space before it. The formatter keeps every comment beside the code it was
written next to. A comment at the end of a line ends that line, so the `{` that
followed it in the source starts the next line instead of joining the one above.

```zql before
// Every centre and their salary
query{
Player(position="C") // centres only
{name // shown first
salary}
}
```

```zql after
// Every centre and their salary
query {
  Player(position = "C") // centres only
  {
    name // shown first
    salary
  }
}
```

### A JSON file

JSON uses the same two-space indent and 80-column target. An array of plain
values stays on one line when it fits, like `["C", "LW"]`. An object with one or
two members stays on one line when it fits, like Alice's; three or more members
get a line each, like Bob's. Key order, numbers and strings are kept exactly as
written: `1.25e7` is not rewritten as `12500000`.

```json before
{"team":"Oilers","country":"CA","founded":1972,"players":[{"name":"Alice","salary":1.25e7},{"name":"Bob","salary":950000,"positions":["C","LW"]}]}
```

```json after
{
  "team": "Oilers",
  "country": "CA",
  "founded": 1972,
  "players": [
    { "name": "Alice", "salary": 1.25e7 },
    {
      "name": "Bob",
      "salary": 950000,
      "positions": ["C", "LW"]
    }
  ]
}
```

## The rules in short

- **Blocks.** `schema`, `unique`, `index`, `mutation`, `query`, `then` and
  `display` always open up, with one blank line between them.
- **Selections and views.** One or two plain items stay inline if they fit;
  three or more, or any nested block, go one per line.
- **Schema types.** One field inline; two or more, one per line, with a blank
  line between types.
- **Spacing.** `name: String`, `playsFor -> Team`, `salary > 10000000`. Nothing
  is lined up in columns.
- **Wrapping.** Filters and arguments stay on one line up to 80 columns, then
  put one item per line between `(` and `)`.
- **Display attributes.** One or two inline, three or more one per line.
- **Comments.** `//` only, kept where they were written.
- **JSON.** Scalar arrays and one- or two-member objects inline if they fit;
  everything else one member per line. Keys, numbers and strings are never
  rewritten.
