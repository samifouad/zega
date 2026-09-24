# Paths

`*1..3` returns everyone within three hops. `*path` returns one route: the
way from this node to the target, and nothing else.

```zql
schema {
  type Junction {
    name: String
    at: Point
    road -> Junction[] { km: Float }
  }
}
unique { Junction { name } }

// Fewest roads.
query { Junction(name: "A") { road *path -> Junction(name: "B") { name } } }

// Fewest kilometres (Dijkstra).
query { Junction(name: "A") { road *path by &km -> Junction(name: "B") { name &km } } }

// Fewest kilometres, searching toward B first (A*).
query { Junction(name: "A") { road *path by &km toward at in km -> Junction(name: "B") { name &km } } }
```

- `*path` counts roads. It is a breadth-first search.
- `by &km` adds up a number stored on the relationship. The field must be an
  `Int` or `Float` declared on it (`road -> Junction[] { km: Float }`).
- `toward at in km` adds a guess: the straight-line distance from each
  junction's `at` to the target's. The search then looks at the junctions
  toward the target first. It returns a route with the same cost as `by &km`
  alone, and it usually reads far fewer junctions.

The selection after the arrow is where the route ends. Its condition can match
several nodes, `Junction(kind: "hospital")`, and the route goes to the
cheapest one. A route that starts on a match costs 0 and has no edges. Every
node after the start must be one of the target's types. It keeps following the
same relationship, so each target type declares it too.

## Result

```json
{
  "road": {
    "cost": 1.6,
    "hops": 2,
    "nodes": [
      { "name": "A", "km": null },
      { "name": "B", "km": 0.8 },
      { "name": "C", "km": 0.8 }
    ],
    "edges": [
      { "id": 1, "type": "road", "from": 1, "to": 2, "props": { "km": 0.8 } },
      { "id": 2, "type": "road", "from": 2, "to": 3, "props": { "km": 0.8 } }
    ]
  }
}
```

- `nodes` runs from the start to the end, in order. Each one is the target
  selection read on that node. `&km` is the road that arrived there, which is
  null on the start. `&hops` is its position.
- `edges[i]` joins `nodes[i]` and `nodes[i + 1]`: the turn-by-turn list. Edges
  have the same shape as the explorer's graph (`id`, `type`, `from`, `to`,
  `props`). `from` and `to` are the stored direction.
- `cost` is the sum of the weights: an `Int` for an `Int` field and a number
  for a `Float` field. For `*path` without a weight it is the number of edges.
  `hops` is always the number of edges.
- The value is `null` when no target can be reached. That is not an error.

Equal routes are broken by node id. Nodes are expanded in order of cost (A*:
cost plus guess), then id. A node keeps the first road that reached it at its
lowest cost. The same data always gives the same route, on native and in the
browser.

## Bounds

```zql
road *path(hops <= 20) -> Junction(name: "B") { name }
road *path(cost <= 50) by &km -> Junction(name: "B") { name }
road *path(cost < 50) by &km toward at in km -> Junction(name: "B") { name }
```

A bound stops the search. A route outside it is `null`. A weighted path is
bounded by `cost`. Without a weight, `hops` and `cost` are the same.

## The unit for A*

The guess is in metres, divided by the unit after `in`: `m`, `km` or `mi`. A*
only returns the cheapest route if the guess never exceeds the real remaining
cost. So the weight has to be a distance in that unit, at least as long as the
straight line between the two ends of its road. Road lengths along the road
meet this; travel times do not. Use `by &minutes` without `toward` for those.

zega checks this on every road A* reads. A road shorter than 99% of its
straight line is an error that names it:

```
error: road#3 from Junction#1 to Junction#4 has km 1.25, shorter than the 1.314 km straight line between its ends
  help: `toward` needs every km to be at least the straight-line distance in km; check the unit, or drop `toward`
```

The 1% margin exists because distances here are measured on a sphere. A road
measured on the WGS84 ellipsoid can be up to 0.56% shorter than the sphere's
straight line. The guess is computed with software trigonometry, so it is the
same on every host.

## Errors

The checker, at the name's span:

- the weight is not a field of the relationship, or is not `Int` or `Float`
- `hops` bound on a weighted path
- `toward` without `by &weight`, or on a field that is not a `Point` on every
  target type
- a target type without the relationship
- `*path` in a mutation, or a target with `near`, `order by` or `limit`
- `toward at` without `in m`, `in km` or `in mi`

At run time, naming the edge or node:

- a road without the weight (`road#2 from Junction#2 to Junction#3 has no km`).
  A missing weight is never read as 0 or 1.
- a negative weight
- with `toward`: a node without its `Point`, or a road shorter than its
  straight line

Each road a search reads costs one unit of the traversal work budget, the same
as `*1..3`. `Zega::nodes_expanded()` (and `nodes_expanded()` in the WASM build)
counts the nodes whose roads a path search has read, which is how the tests
show A* reads fewer than Dijkstra.
