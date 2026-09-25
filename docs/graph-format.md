# The `.graph` file format

A `.graph` file holds one whole graph: its nodes, relationships, ids,
properties, index declarations, and optionally the schema it was written
with. Every zega surface imports and exports it: the Rust API, the CLI, the
`zega start` HTTP server and the browser build (wasm). It is the interchange
format between them and between zega versions, and the unit that packs
(APS 17), trials moving to Plus (APS 18) and shares (APS 19) carry.

This document is the specification. `zega/src/graph_file/` is the reference
implementation, and `zega/tests/fixtures/golden-v1.graph` is a frozen version 1
file that every later zega must keep importing.

- Media type: `application/vnd.zega.graph`
- File extension: `.graph`
- Current format version: **1**

## Design

**The format version is independent of the engine version.** It changes
only when these bytes change. A file records which engine wrote it (for
diagnosis), but readers never branch on that.

**Encoding: a small custom binary format, little-endian and fixed-width,
with a string dictionary.** The alternatives were weighed against the three
hard requirements:

| requirement | why the custom binary format meets it | the alternatives |
|---|---|---|
| **Streams in bounded memory** both ways | A file is a sequence of sections, and a section is a sequence of self-delimiting records. A reader decodes one record at a time; a writer emits one record at a time. Each section header carries the payload length, so the writer sizes a section with a dry run of the same encoder (no buffering) before writing it. | JSON and CBOR stream too, but a streaming CBOR or JSON *writer* with a deterministic map order and a per-section checksum is exactly this design with a heavier encoding underneath. Protobuf can't frame a million records without an outer framing of its own. |
| **Simple in JS/TS**, so a Cloudflare Worker can read it without the engine | Every field is `u8`, `u32`, `u64`, `i64`, `f64` bits or a length-prefixed UTF-8 string: `DataView` plus `TextDecoder`. There are no varints, no schema compiler and no library. The checksum is CRC-32 (a 10-line table), and the content digest is SHA-256, which Workers stream with `crypto.DigestStream`. | CBOR and MessagePack need a library, and deterministic CBOR (RFC 8949 §4.2) must be enforced by hand on top of one. Arrow/Parquet are the right shape for columns (APS 20) but heavy for an interchange file. |
| **Deterministic**: the same graph gives the same bytes | There is exactly one encoding of each graph. Ids ascend, map keys ascend, names are a sorted dictionary, floats are stored as their exact bits, and there are no optional encodings (no varints, no alternative widths). Readers **reject** anything else, so an accepted file exports back to exactly itself. | JSON has many spellings of one number, and floats round-trip through text only with care (NaN, `-0.0` and infinities not at all). |

**What it costs.** Fixed-width integers make a file larger than a varint
encoding: the 1M-node, 1M-relationship test graph is 78 MB. It gzips well,
and HTTP layers compress it. Compression is left to the transport rather
than built in, so a JS reader needs no decompressor and the bytes stay
hashable as written.

**Interchange, not storage.** APS 20's segment files are the storage format:
columnar, mmapped and indexed. A `.graph` file is the row-oriented, portable
form that any engine version can produce and consume. Segments can be built
from a `.graph` file and written back to one.

## Primitive types

All integers are little-endian.

| type | bytes | meaning |
|---|---|---|
| `u8` | 1 | unsigned |
| `u32` | 4 | unsigned |
| `u64` | 8 | unsigned. JS: `DataView.getBigUint64` |
| `i64` | 8 | two's complement. JS: `DataView.getBigInt64` |
| `f64` | 8 | the IEEE 754 bit pattern, stored as a `u64`. Every bit pattern is preserved, NaN payloads included. **Read it as raw bits** (JS: `getBigUint64`, not `getFloat64`): converting a NaN to a JS number may change its payload, and the bits are what round-trip |
| `f32` | 4 | the IEEE 754 bit pattern, stored as a `u32` (JS: `getUint32`) |
| `str` | 4 + n | `u32` byte length, then that many bytes of UTF-8. Invalid UTF-8 is an error |
| `name` | 4 | `u32` index into the file's name dictionary |

**Order.** Wherever this document says strings are "ascending", it means
ascending by their UTF-8 bytes, compared as unsigned bytes (`memcmp`). That
is not JS's `<` on strings, which compares UTF-16 code units: sort
`TextEncoder` output, not the strings.

## Layout

```text
file    = magic version manifest names schema nodes relationships done
magic   = 89 5A 47 52 41 50 48 0A        ("\x89ZGRAPH\n")
version = u32                            (1)
section = tag:4 bytes  length:u64  payload:length bytes  crc:u32
```

| # | tag | section | content digest |
|---|---|---|---|
| 1 | `MNFT` | manifest | no |
| 2 | `NAME` | names | yes |
| 3 | `SCHM` | schema | yes |
| 4 | `NODE` | nodes | yes |
| 5 | `RELS` | relationships | yes |
| 6 | `DONE` | done | no |

- The magic's first byte is outside ASCII and it ends in a newline, so a file
  mangled by a text-mode transfer fails the magic check.
- The sections appear exactly once each, in this order. Nothing may follow
  `DONE`.
- `crc` is the CRC-32 (IEEE 802.3, as in zlib and PNG) of the section's
  payload, not including its tag or length.
- A payload must be consumed exactly by its records: bytes left over, or a
  record that runs past `length`, make the file invalid.

### Manifest (`MNFT`)

```text
created_by      str   the writer, e.g. "zega 0.2.0"; informational only
node_count      u64
rel_count       u64
meta_count      u32
meta_count × { key: str, value: str }   keys unique, ascending by bytes
```

Readers use the counts to know how many records to read. The graph's id
counters are not here: they are graph content, so they sit at the start of
the `NODE` and `RELS` sections, inside the content digest.

The metadata is free-form text. These keys are defined (APS 17's provenance
and licence at file level):

| key | value |
|---|---|
| `title` | a human name for the graph |
| `source` | where the data came from |
| `source_version` | the version of that source, e.g. a pack version |
| `licence` | an SPDX identifier or a licence name |
| `fetched_at` | RFC 3339 time the data was taken |

Other keys are allowed and preserved. Per-node and per-relationship
provenance (APS 17, requirement 3) is future work.

### Names (`NAME`)

```text
count   u32
count × str     unique, ascending by bytes
```

The dictionary holds every node label, relationship kind and top-level
property key used in the file, and nothing else: a name that no record uses
makes the file invalid. A `name` field refers to an entry by its position.
Keys inside `map` values are written inline, not through the dictionary.

### Schema (`SCHM`)

```text
has_source   u8        0 or 1
source       str       only if has_source = 1: ZQL schema text, verbatim
index_count  u32
index_count × { kind: u8, type: str, field: str }
             sorted by (type, field, kind); kind 0 = range, 1 = text
unique_count u32
unique_count × { type: str, field: str }
             sorted by (type, field)
```

- **Schema source** is ZQL text carried with the graph: given at export
  (the CLI's `--schema`, `exportGraph(schema)`), or kept from the file the
  graph was imported from. A zega database takes a schema with every query
  and has none of its own, so it keeps an imported file's schema section and
  manifest metadata as graph state (in memory, through the WAL and in
  snapshots) and writes them out again on export. Import then export gives
  back the same file. A graph that never had a schema writes
  `has_source = 0`.
- **Index declarations** are the indexes that schema declares, as the engine
  reads it: its `index { }` blocks, plus the range index every orderable
  `unique` field gets. An importer declares them and builds them from the
  nodes, so the graph is indexed as soon as it lands. Index *contents* are
  never stored, because they derive from the data. The indexes a running
  database has declared are not written: they follow whichever schema the
  last query ran with, and a restart forgets them. So they're session
  state, and the same graph must export the same bytes before and after a
  restart.
- **Unique constraints** are the `unique { }` blocks of that source, listed
  so a reader without a ZQL parser can see them. The writer derives them from
  the source with the same parser the engine uses, so the two cannot
  disagree.
- **Declarations need their source.** A file with `has_source = 0` and any
  index or unique declaration is invalid. A reader keeps the declarations
  as written; it does not re-derive them, so a file stays readable even if a
  later ZQL grammar would read its schema differently.

### Nodes (`NODE`)

```text
next_node_id u64          the id the next created node will get
node_count × node         in ascending id order

node:
id           u64          below next_node_id
label_count  u32          0 or more
label_count × name        in the node's own order (the first is its type);
                          no label twice
prop_count   u32
prop_count × { key: name, value }   ascending by key
```

`next_node_id` travels with the graph, so an imported graph gives the next
new node the id the original would have. A node may have no labels. A label
listed twice on one node makes the file invalid (the engine treats labels as
a set, so a repeat would not survive a round trip).

### Relationships (`RELS`)

```text
next_rel_id  u64          the id the next created relationship will get
rel_count × relationship  in ascending id order

relationship:
id           u64          below next_rel_id
kind         name
from         u64          a node id in this file
to           u64          a node id in this file
prop_count   u32
prop_count × { key: name, value }   ascending by key
```

A relationship whose `from` or `to` is not a node in the file makes the
file invalid.

### Values

A value is a one-byte tag and a payload. Every `Value` variant of the engine
(`zega/src/value.rs`) has one encoding:

| tag | variant | payload | rules |
|---|---|---|---|
| `0x00` | Null | none | |
| `0x01` | Bool false | none | |
| `0x02` | Bool true | none | |
| `0x03` | Int | `i64` | |
| `0x04` | Float | `f64` bits | any bit pattern, stored exactly |
| `0x05` | String | `str` | |
| `0x06` | List | `u32` count, then count × value | |
| `0x07` | Map | `u32` count, then count × { key: `str`, value } | keys unique, ascending by bytes |
| `0x08` | Point | lat `f64`, lon `f64` | WGS84 degrees; lat in [-90, 90], lon in [-180, 180], finite, `-0.0` stored as `0.0` |
| `0x09` | Vector | metric `u8`, `u32` dimensions, dimensions × `f32` | metric 0 = cosine, 1 = dot, 2 = l2; 1 to 4096 dimensions; finite; `-0.0` stored as `0.0` |

Lists and maps nest at most 128 deep (ZQL's own nesting limit, and deeper
than any JSON load produces). A reader rejects unknown tags.

### Done (`DONE`)

```text
content_sha256   32 bytes
```

The SHA-256 of every byte from the first byte of the `NAME` section's tag to
the last byte of the `RELS` section's `crc`. That range holds the names, the
schema section, every node and relationship, and both id counters.

The digest makes truncation after the last data section detectable. It is
also the graph's **content id**: two files hold the same graph (with the same
id counters and the same schema section) exactly when their digests match,
whatever wrote them and whatever metadata they carry. APS 19 share links and APS 17 pack caches can
key on it. The whole file is deterministic too, but it includes the
manifest, so it also changes with `created_by` and the metadata.

## Reading rules

A reader must refuse a file, and import nothing, when:

| condition | zega's error |
|---|---|
| the first 8 bytes are not the magic | `not a .graph file: it does not start with the .graph magic bytes` |
| the version is newer than it supports | `this file is .graph format version N; this zega reads versions 1 to M. Upgrade zega to import it` |
| the input ends early, anywhere, including exactly at a section boundary | `truncated .graph file: it ends at byte B, where the nodes section should start` (or `inside the … section`) |
| a section's CRC-32 does not match | `corrupt .graph file: the nodes section's checksum is X, the file says Y` |
| the content digest does not match, a rule above is broken, a resource limit below is exceeded, or bytes follow `DONE` | `invalid .graph file: <reason> (byte B, <section> section)` |

When a record fails to decode, zega reads the rest of that section and
checks its CRC first. So damage in transit is reported as a checksum error,
not as whatever the damaged bytes happened to look like.

### Resource limits

A file's counts and lengths are claims until their bytes arrive, so a
reader must not trust them with memory. zega's reader:

- reserves at most 16 items (4 KiB for a string) ahead of what it has read,
  so collections grow with the bytes actually delivered. A count that could
  not fit in what remains of its section is refused before anything is
  read;
- refuses lists and maps nested more than 128 deep, with two small stack
  frames per level, so the limit holds on wasm's 1 MiB stack;
- never holds a section or the file: memory is the graph being built plus a
  64 KiB buffer.

A limit hit is an `invalid .graph file` error naming it, e.g. `a value
nests lists and maps more than 128 deep`. Tested: every hostile file of up
to 4 KB in `zega/tests/graph_file_hostile.rs` (claimed counts of 2^32−1,
lengths of 2^62, 400-deep nesting, 20,000 mutated golden files) is refused
with under 4 MiB of heap. `zega start` also caps an upload's size
(`--max-import-bytes`, default 64 MiB: decoding takes 12–22× a file's size).

### Compatibility

- A reader accepts every format version from 1 up to the newest it knows,
  and refuses newer versions with the error above. It never guesses at a
  newer file.
- Every change to these bytes, even an addition, is a new format version.
  There is no "ignore unknown sections" rule: a reader that can't be sure it
  understood a file refuses it.
- A writer emits the **lowest version that can represent the graph**, so
  files stay readable by as many zegas as possible. Today that is always 1.
  When version 2 adds something (say, per-node provenance), a graph that
  doesn't use it is still written as version 1.
- Each version keeps a golden file in `zega/tests/fixtures/`
  (`golden-v1.graph`, …), and a test imports each one on every build. When
  version 2 is introduced, the test that the writer still produces
  `golden-v1.graph` byte for byte is retired, and the read test stays.

## zega's implementation

| surface | export | import |
|---|---|---|
| Rust | `Zega::export(&mut impl Write)`, `Zega::export_with(out, &ExportOptions)` | `Zega::import(impl Read) -> ImportSummary` |
| CLI | `zega export g.graph [--schema s.zql] [--meta k=v]…` (`-` = stdout) | `zega import g.graph [--replace]` (`-` = stdin) |
| HTTP (`zega start`) | `GET /graph` serves the file; a client that prefers `application/json` (by `Accept` q-values) gets the explorer's JSON view instead. Responses carry `Vary: Accept`; `406` if neither is acceptable | `PUT /graph` with the file as the body; answers `{ "ok": true, "result": <summary> }`. `413` over `--max-import-bytes` (default 64 MiB, refused up front when `Content-Length` says so), `408` after 30 s without data. `DELETE /graph` replaces the graph with an empty one, dropping what an import carried |
| wasm | `db.exportGraph(schema?, metaJson?)` returns a `Uint8Array` | `db.importGraph(bytes)` returns the summary as JSON |

- **Export streams.** It holds the database's lock while it writes, so
  writers wait, but it never copies the graph or buffers the file. `zega
  start` writes the export to a staging file under the lock and releases it
  before sending a byte, and spools an upload to a staging file before
  taking the lock. A slow or stalled client costs disk space, never the
  database, and not for long: a transfer that makes no progress for 30 s
  is dropped with its staging file, and at most 16 transfers hold staging
  files at once (more get `503`). Beyond
  the graph it uses the name dictionary and a 64 KiB buffer (a dense id range
  is walked in order; a sparse one sorts a copy of its ids). Measured on
  1M nodes and 1M relationships: 0.06 MiB of heap beyond the graph, against
  2.6× the graph for the JSON `GET /graph` it replaces (#52:
  2.3× RSS on Fly).
- **Import is all-or-nothing.** The file is decoded and checked to its last
  byte into a new graph while the old one keeps serving. The new graph
  replaces the old one only when the file has fully passed. Peak memory is
  the old graph plus the new one, plus a buffer.
- **Durable imports.** A disk database first writes the incoming bytes to
  `graphs/.incoming-….tmp` beside its WAL. Once the file has passed, it
  syncs it, renames it to `graphs/<sha256 of the file>.graph`, and appends
  one WAL entry (`ReplaceGraph`) naming it. That entry is the commit point:
  a crash before it leaves the old graph, and a crash after it replays to the
  new one. WAL entries after it replay on top as usual.
- **Replay starts at the last import.** An import replaces everything
  before it, so opening a database reads only the last imported file and
  the WAL entries after it, whatever came earlier. The previous import's
  file is deleted as soon as the next import commits, and opening deletes
  any unreferenced `graphs/*.graph` and staging files a crash left.
  Imports into one database commit one at a time, so concurrent imports
  never delete each other's files. Staging and imported files are created
  owner-only (mode 0600). (The
  WAL itself still holds the earlier entries until WAL compaction, #52.)
- **Downgrading is a one-way door.** Before its first `ReplaceGraph` entry,
  zega marks the WAL header version 3. A zega that predates `.graph` refuses
  that WAL with `unsupported WAL version 3` rather than misreading it. To go
  back to an older zega, export with the new one and rebuild the data
  another way; the older one can't import `.graph`.
- **Import replaces the whole graph.** Merging a file into an existing
  graph by global id is APS 17's upsert, and future work. The CLI refuses to
  replace a non-empty database without `--replace`.

## An empty graph

The whole file for a graph with no nodes, no relationships, no schema and no
metadata, written by zega 0.2.0 (203 bytes):

```text
00000000: 89 5a 47 52 41 50 48 0a 01 00 00 00 4d 4e 46 54  magic, version 1, MNFT
00000010: 22 00 00 00 00 00 00 00 0a 00 00 00 7a 65 67 61  length 34; "zega
00000020: 20 30 2e 32 2e 30 00 00 00 00 00 00 00 00 00 00   0.2.0", 0 nodes,
00000030: 00 00 00 00 00 00 00 00 00 00 10 8a 69 0f 4e 41  0 rels, 0 meta; crc; NA
00000040: 4d 45 04 00 00 00 00 00 00 00 00 00 00 00 1c df  ME, length 4: 0 names
00000050: 44 21 53 43 48 4d 09 00 00 00 00 00 00 00 00 00  SCHM, length 9: no source,
00000060: 00 00 00 00 00 00 00 ae 14 09 e6 4e 4f 44 45 08  0 indexes, 0 uniques; NODE
00000070: 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00 f7  length 8: next node id 1
00000080: df 88 a9 52 45 4c 53 08 00 00 00 00 00 00 00 01  RELS, length 8: next rel
00000090: 00 00 00 00 00 00 00 f7 df 88 a9 44 4f 4e 45 20  id 1; DONE, length 32:
000000a0: 00 00 00 00 00 00 00 a4 6b 9d 6f b9 d3 d1 4b 55  the content digest
000000b0: db 17 a1 0f 5f 1f b2 f1 d5 5e b7 2b 02 a3 61 5a
000000c0: d0 1f c1 66 f9 50 c5 c6 97 21 73                 and the DONE crc
```

## A reader in outline

This is what a Worker-side reader does. It isn't part of this repository yet.

```js
const u32 = () => { const v = view.getUint32(at, true); at += 4; return v; };
const u64 = () => { const v = view.getBigUint64(at, true); at += 8; return v; };  // also f64 bits
const str = () => { const n = u32(); const s = utf8.decode(bytes.subarray(at, at + n)); at += n; return s; };
// magic, version ≤ 1, then for each section:
//   tag (4 bytes), length (u64), payload, crc32(payload) === u32()
// NAME → names[]; NODE → { id: u64(), labels: [...u32()].map(i => names[i]), props };
// value: switch (tag) { 0: null, 1: false, 2: true, 3: getBigInt64, 4: u64() as raw bits, … }
```
