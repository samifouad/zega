# ZQL data loading

```zql
schema {
  type Player {
    name: String
    salary: Int
  }
}

mutation csv ["./players.csv"] {
  Player(name: $Name && salary: $Salary) {
    name
    salary
  }
}

mutation json ["https://example.com/players.json"] {
  Player(name: $Name && salary: $Salary) {
    name
    salary
  }
}
```

JSON accepts an object or an array of objects. CSV uses a header row and the
Rust `csv` crate, including quoted commas, escaped quotes and multiline cells.
Records must have the same number of fields as the header; headers must be
nonempty and unique. CSV cells retain ZQL's existing scalar inference (null,
boolean, integer, decimal, otherwise string). JSON retains its original types.
Use `$"Full Name"` for a column containing spaces. Normal mutation rules,
including required fields, unique constraints and relationship linking, apply.

Native `Zega::run_lang` and `Zega::apply_zql` read files relative to the **process
working directory**, for both in-memory and disk databases. Absolute paths are
also accepted. The existing prohibition on `..` path components remains. The
HTTP client is blocking `ureq` with rustls, requires no Tokio runtime, ignores
proxy environment variables, has a 20-second request timeout and follows at
most five redirects. Imports are limited to 2,000,000 UTF-8 bytes per source;
reads stop at that limit instead of buffering an unbounded response.

The `http` Cargo feature is enabled by default. `default-features = false`
disables native HTTP while retaining local-file and supplied-source imports.
HTTP dependencies and native I/O are excluded on wasm32, even under
`--all-features`; `zega-wasm` explicitly disables default features.

Public HTTP(S) addresses are accepted by default. Private/loopback hosts and
resolved private IPs are rejected, including redirect targets. Trusted native
callers can explicitly build with `.allow_private_imports(true)`. This allows
local fixture servers without changing the default conformance policy. URL
credentials and non-HTTP schemes are rejected. This is not a filesystem sandbox:
callers authorized to execute ZQL may read files accessible to the process.

Fetching and parsing happen before taking the graph lock. Rows insert under
that lock through the same mutation and WAL operations as handwritten ZQL.
A malformed source is rejected before insertion. A load is one statement, and
every statement is all-or-nothing: if any row fails a constraint, or the WAL
refuses the write, no row of the load is kept, in memory or in the WAL.

## Browser hosts

Wasm performs no file I/O or blocking HTTP. Hosts call `zql_load_locations` to
get validated locations, fetch raw text asynchronously (or use a file picker),
and call `run_lang_with_sources` / `apply_zql_with_sources` with a map from each
literal ZQL location to its raw UTF-8 text. Missing entries are errors; there is
no network fallback. `parse_import` exposes the same Rust parser for previews.
The corresponding wasm methods serialize the source map as a JSON envelope;
that envelope does not parse or convert data rows in JavaScript.

The explorer uses JS `fetch` / `File.text` for transport and Rust for both
preview parsing and insertion. Its Import UI maps column names to a ZQL load
template and sends the original text to the engine. JS renders the Rust preview
and constructs the template, but does not parse JSON/CSV rows, coerce cells,
generate one literal mutation per row, or silently cap imports at 150 rows.
Imports now have actual ZQL semantics: one graph per row, including repeated
values; use explicit ZQL `link` to connect to an existing unique node.

## Before this change (c14fbd3)

| Consumer | Local file reader | Remote fetcher | JSON parser | CSV parser |
| --- | --- | --- | --- | --- |
| Native library, disk | Rust std::fs, process cwd | Rust reqwest::blocking | serde_json + json_rows | handwritten Rust parser |
| Native library, memory | Same as disk | Same as disk | Same as disk | Same as disk |
| zega-server | Not exposed through its old /cql API | Not exposed through /cql | Only HTTP request envelopes | No load API |
| zega-wasm | No disk access; relative locations are browser URLs | Synchronous XHR via web-sys | Rust serde_json | handwritten Rust parser |
| Explorer ZQL editor | Relative browser URLs via wasm | wasm XHR | Rust engine | Rust engine |
| Explorer Import dialog | JS File.text/drop/clipboard | JS fetch | JS JSON.parse/jsonTable | JS parseCsv |

The library already inserted imports through the WAL. The explorer Import
dialog previously generated literal mutations from JS-parsed rows. The CLI PR
replaces the old server API with ZQL so native server callers use this loader.
