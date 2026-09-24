# Discovery stages (APS 7)

[APS 7](https://github.com/zegadb/aps/issues/7) adds a query pipeline:

```zql
query { Player { name playsFor -> Team { name } } }
display { skip }
then { common { Player { country } Team { country } } && findWith { "Oilers" } }
then { startsWith { "A" in { name } } || regex { "ers$" } }
```

A `then` has one condition. `&&` binds more tightly than `||`; parentheses
change precedence. Both operands inspect the same previous stage, independently.
Each returns nodes and possibly proposed edges. Intersection retains only nodes
in both sets and edges with both ends remaining. Union retains either set and
its edges, deduplicated. A subsequent stage sees only the preceding node set;
proposals are recomputed by that stage, not inherited. Discovery never writes.
A proposal is evidence for a possible relationship, not a relationship type or
permission to link it. The explorer can use it to prepare a mutation for review.

Stage scope includes every distinct node actually projected by the query:
roots, selected nested relationships, and the nodes of returned `*path` routes.
It excludes the traversal search frontier and nodes excluded by filters/limits.
Selecting `@id` is unnecessary. Discovery reads stored fields on these identities,
including fields not projected by the first query. Type checking uses all types
reachable in that query's selections, conservatively across later stages.

`then` without a query, after a mutation, or a discovery sub-block in a filter
is an error. The existing shorthand read `{ Player { name } }` can also start
a pipeline. The text operators still work infix in filters:
`Player(name findWith "Oilers")`. Inside `then`, write
`findWith { "Oilers" }`; a bare infix operator has no implicit field.
Language blocks are bare, following [APS 6](https://github.com/zegadb/aps/issues/6).
Named scopes use declaration-style field lists; `similar` and `near` use `&field`.
Types and fields named `findWith`, `startsWith`, etc. remain usable.

## Text

`findWith`, `findWithout`, `startsWith`, `endsWith`, and `regex` accept one
string and an optional `in { name bio }` field scope. Matching is case-sensitive
Unicode text, without normalization. Positive operations match if **any** eligible
String field matches. `findWithout` is the complement inside the input set:
a node with no present String fields also matches it. Null/missing fields do
not match a positive operation. Empty text follows Rust string semantics.

A scoped field must exist on at least one input type; every input type that
declares it must declare it as String. Types without it contribute no match.
Unscoped operations inspect only String properties, never relationships or
stringified numbers. An unscoped query whose result types have no String fields
is rejected.

Text indexes accelerate substring, prefix and suffix candidates, including
matching candidates removed by `findWithout`; exact matching verifies them.
Scopes containing unindexed fields safely scan those fields. Needles shorter
than a trigram retain the existing index behavior. Regex uses the Rust `regex`
crate, compiled once per atom when parsing a query; execution reuses the compiled
pattern for every node. Lookaround and backreferences are unsupported and the
error says so. Rust's default compilation size limits apply. **Follow-up:**
required-literal extraction for regex/trigram acceleration. Regex currently scans
the input String fields; it never substitutes an unsafe literal approximation.

## Shared values

`common { Player { country } Team { country } }` groups by equal values across
these types. Multiple fields form an ordered tuple: `A { country year }` can
match `B { nation season }` when the declared types match position by position.
All tuples must have the same arity and declared types. Missing/null values do
not form groups. Singletons are excluded. Field names in the evidence are
qualified (`Player.country`) and sorted; `value` is a scalar for one field and
an array for a tuple. Float negative zero equals positive zero.

**Fan-out rule:** a group of N distinct node IDs emits exactly N-1 edges,
from its smallest ID to every other member. This stable star has linear size;
it is not every possible pair. Filtering a star's center out with `&&` removes
its edges; it does not invent replacement edges among the remaining nodes.

## Similarity and proximity

`similar { &embedding > 0.9 }` selects nodes participating in a qualifying
pair of distinct input nodes. `>=` is also accepted. Fields must be Vector with
matching dimensions and metrics on every input type that declares that field.
Absent/null vectors do not participate. Scores follow the schema metric:
cosine, dot product, or negative Euclidean distance (larger is better).

The existing HNSW index supplies scores when present. Threshold discovery asks
for the complete eligible set rather than silently truncating to a nearest-k.
Any unreturned candidate is scored exactly, which also supplies the no-index
fallback. This preserves exact threshold semantics and can cost O(N²) for dense
input. Pair work and output remain subject to the engine traversal work budget.
No approximate recall contract is introduced by `then`.

`near { &at < 1 km }` (also `<=`) selects nodes in qualifying Point pairs.
Distances must be finite and non-negative, with an explicit `m`, `km`, or `mi`
unit. Evidence distance is always **metres**, regardless of input unit.
The spatial index provides conservative candidates; exact spherical haversine
using software trigonometry supplies selection and output, identical in native
and WASM. Null/missing Points do not participate. No node pairs with itself.

## Response contract

Without `then`, the existing JSON result remains unchanged. The explicit
`display { skip }` alone suppresses that result and returns null. With `then`,
the result is an object containing `stages`:

```json
{
  "stages": [
    {
      "index": 0,
      "kind": "query",
      "nodes": [{"id": 1, "labels": ["Player"], "props": {"name": "Alice"}}],
      "edges": []
    },
    {
      "index": 1,
      "kind": "then",
      "nodes": [],
      "edges": [],
      "couldBeEdges": []
    }
  ]
}
```

Stage nodes carry stable graph identities, labels and stored properties in a
separate `props` object, so user fields such as `id` and `labels` cannot overwrite
identity. This is the discovery graph format; ordinary query projections are
unchanged. Real `edges` are the query's projected relationships with both ends
in that stage, using the existing `{id,type,from,to,props}` graph edge format.
`couldBeEdges` are separate; they have no stored relationship ID:

```json
{"from":1,"to":2,"via":{"primitive":"common","fields":["Player.country","Team.country"],"value":"CA"}}
{"from":1,"to":3,"via":{"primitive":"similar","fields":["embedding"],"score":0.95}}
{"from":1,"to":4,"via":{"primitive":"near","fields":["at"],"distance":250.0}}
```

Pairs are undirected evidence, serialized with `from < to`. Different primitive
or value evidence for one pair remains separate. Nodes are sorted by numeric ID;
real edges by numeric relationship ID; proposals by numeric from/to then canonical
JSON of `via`. Identical proposals are deduplicated. No iteration-order dependence.

`display { skip }` binds immediately to the preceding query or then. That stage
is computed and checked but its **entire entry is omitted**, including nodes,
real edges and proposals. Indices retain their original zero-based positions.
All skipped yields `{"stages":[]}`. Clients can distinguish earlier visible
stages from the latest visible stage by index; blur/highlight/dashed-edge drawing
belongs to explorer part 2, outside this change.

These details (tuple equality, null behavior, star center, exact thresholds,
response field names, graph envelopes, units and ordering) are implementation
choices where APS 7 leaves the contract open. They are covered by native and
browser conformance tests.
