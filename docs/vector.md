# Vectors and meaning views

A Vector is a fixed-size array of finite float32 numbers. The dimension is
1 through 4096. Values are rounded once to float32 on input; results report
those stored values. A wrong dimension, non-number or float32 overflow is a
type error with a source span. Optional fields use `embedding?: Vector<384>`.

```zql
schema {
  type Ticket {
    title: String
    status: String
    embedding: Vector<384>
    related -> Ticket[]
  }
  display {
    vector2d { Ticket }: Default
    vector3d { Ticket }
    table
  }
}
```

The default metric is cosine. `Vector<384, dot>` selects dot product;
`Vector<384, l2>` selects Euclidean distance. All scores are larger-is-better:
cosine in [-1, 1], raw dot product, or **negative** Euclidean distance.
Cosine involving a zero vector is defined as zero. Ties break by ascending
node ID. No model is run by the index: text and embeddings are separate fields.

## Write and load

```zql
schema {
  type Ticket {
    title: String
    embedding: Vector<3>
  }
}

mutation {
  Ticket(title: "Reset password" && embedding: @vector[0.8, 0.2, 0.1]) { @id }
}

mutation {
  Ticket(title: "Reset password") set embedding: @vector[1, 0, 0] { embedding }
}
```

JSON loads accept arrays under the declared field name, or explicit bindings:

```json
[{"title":"Reset password","embedding":[0.8,0.2,0.1]}]
```

```zql
mutation json ["tickets.json"] {
  Ticket(title: $title && embedding: $embedding) { @id }
}
```

CSV requires an explicit mapping of exactly N numeric columns:

```zql
schema {
  type Ticket {
    title: String
    embedding: Vector<3> from (x, y, z)
  }
}

mutation csv ["tickets.csv"] {
  Ticket(title: $title) {
    @id
    embedding
  }
}
```

No coordinate or embedding columns are guessed. The mapping also works with
JSON. An explicit assignment wins, then a named JSON array, then mapped
columns. All rows are bound and checked before a load writes any of them.

## Nearest and threshold queries

These use the existing selection syntax. `@near` follows the optional filter,
and precedes the fields. Select `@score` or alias it as `relevance: @score`.

```zql
query {
  Ticket(status = "Open") @near(embedding, @vector[1, 0, 0], 10) {
    @id
    title
    @score
    related -> Ticket {
      @id
      title
    }
  }
}

// Explicit exact scan, including all matching vectors.
query {
  Ticket @near(embedding, @vector[1, 0, 0], 10, exact) {
    @id
    title
    @score
  }
}

// Full-vector threshold, combinable with && and ||.
query {
  Ticket(@similarity(embedding, @vector[1, 0, 0]) >= 0.8 && status = "Open") {
    @id
    title
    relevance: @similarity(embedding, @vector[1, 0, 0])
  }
}

// Nearest within the nodes reached by a real relationship.
query {
  Ticket(title = "Reset password") {
    related -> Ticket @near(embedding, @vector[1, 0, 0], 5) {
      @id
      title
      @score
    }
  }
}
```

The query vector must have the field's dimension. The examples above use
`Vector<3>`; a `Vector<384>` query needs 384 components. ZQL's `$name` syntax
remains an import binding, so queries write `@vector[...]` literals. Nearest
selections return arrays, including with equality filters. `@near` is read-only
and cannot be combined with distance ordering. Missing optional vectors are
excluded. An additional `limit` can truncate the nearest result.

Nearest searches use HNSW by default. Ordinary filters and relationship
traversals restrict eligible results before nearest-k is chosen. Search
widens if filtering leaves too few results, and finishes as an exact scan
once it has scored half the indexed vectors. Threshold
filters evaluate full-vector scores by scanning candidates, so they do not
silently discard threshold matches. `exact` always scans the full eligible
set, with the same scores and tie order.

## Index and persistence

Every Vector field automatically maintains an in-tree Rust HNSW index, with
no C/C++ dependencies, threads or network access. It runs on native and wasm32.
The implementation follows the layered greedy search, bounded best-first
search and diversity heuristic in [Malkov and Yashunin's HNSW paper](https://arxiv.org/abs/1603.09320).
M=24, the bottom layer permits 48 links and construction ef=160. A fixed seed
and node IDs determine levels; ties are stable. Index partitions separate
field names, dimensions and metrics.

The search width adapts to the query. It starts at max(k, 64) and doubles
until two successive widths return the same top k; a wider pass continues the
narrower one and never scores a vector twice. Once half the vectors are scored,
the search completes as an exact scan, reusing those scores.

What that guarantees depends on the data's *relative contrast*: the mean
distance to all vectors over the distance to the k-th nearest.

- **Contrast of about 2 or more** (clustered data, and real embeddings): at
  N = 100,000 and k = 10, the target is recall@10 of at least 0.95 while
  scoring at most 10% of the vectors per query.
- **Low contrast** (below about 2, such as uniform noise in 128 or more
  dimensions): the nearest vectors are barely nearer than the rest, and no index
  can skip most of the data. The guarantee is recall@10 of at least 0.95 at
  **never more work than an exact scan**.

Insert and replacement update the index. Deletes immediately remove eligibility;
tombstones remain routing nodes until a deterministic rebuild when over half
the allocated entries are dead (above 64 entries). Updates remove the previous
entry and insert the new vector. WAL and snapshots store float32 values and
metrics; replay rebuilds the index through normal writes, as for Point. Snapshot
restoration inserts nodes in ID order. Rebuilding can change approximate
neighbors after a history of updates; exact results remain identical.

The regression test measures recall@10 against independent brute force on
10,000 seeded uniform 128-dimensional vectors and 40 held-out queries. It
requires at least 0.95 recall, and checks exact IDs **and scores** against the
reference; it measures **0.9875 recall@10** (395/400 neighbors). A second test
bounds both recall (at least 0.95) and the vectors scored per query on 6,000
seeded 16-dimensional vectors, uniform and clustered.

Measured at N = 100,000, k = 10, 100 held-out queries:

| data | dims | contrast | recall@10 | scored per query |
|---|---|---|---|---|
| 40 clusters | 32 / 128 / 384 | 179–302 | 0.999 / 0.998 / 0.993 | 1.8% / 2.2% / 2.3% |
| uniform | 32 | 2.6 | 0.995 | 5.1% |
| uniform | 128 | 1.5 | 0.959 | 37% (hard queries become scans) |
| uniform | 384 | 1.2 | 0.970 | 89% (hard queries become scans) |

Approximate recall is data-dependent; the measured corpora are not a guarantee
for every dataset.

## Explorer

Only explicitly declared views are offered. Every type in a vector view must
have a Vector field. With no type list, every declared type must qualify.
The first Vector field in schema order supplies that type's points. Optional
missing values have no point. Select `@id` in the query: the engine projects
exactly the result IDs, including nested relationship results, and fetches
their stored vectors. Unselected nodes do not participate.

The engine's `vector_view` API returns the query result alongside PCA metadata.
Centered PCA computes up to three axes using deterministic power iteration,
orthogonal deflation and stable signs. Both views use the same three-axis
projection; 2D shows its first two axes. Incompatible dimensions or metrics
form independent projection groups, whose positions should not be compared.
PCA positions are approximate; the UI states that scores use full vectors.

- **vector2d**: drag to pan, scroll to zoom; click a point to open the shared inspector.
- **vector3d**: drag to rotate, Shift-drag to pan, scroll to zoom.
- Choose a scalar colour field; Search glow highlights matching text.
- **Similar to this** lists exact nearest neighbors within the plotted result set,
  excluding the selected node, with scores and highlighted points.
- **Links vs meaning** draws actual relationships. A linked pair below the chosen
  score threshold is *linked-but-far*; an unlinked pair at or above it is
  *near-but-unlinked*. Links in either direction count. Flags use full vectors,
  never projected distance. Adjust the threshold for dot or negative-L2 scores.

Rendering uses the browser's native Canvas API and repository JavaScript;
there is no renderer CDN or remote projection service. The **Tickets** sample
has 200 synthetic support tickets across five topics, with deliberate
cross-topic relationships. Its vectors are synthetic, **not from a model**.
Regenerate exactly with `node browser/scripts/tickets-sample.mjs`.
