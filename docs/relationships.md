# Relationships

A relationship connects two nodes. In the schema it is a field with an arrow;
in a query you follow it by naming that field, with the arrow, and selecting
from the node at the other end.

```zql
schema {
  type Team {
    name: String
    players: MEMBER <- Player[]
  }

  type Player {
    name: String
    position: String
    playsFor: MEMBER -> Team {
      since: Int
      captain?: Bool
    }
    mentors -> Player[]
  }
}
```

## Direction

Every relationship has a direction. `playsFor: MEMBER -> Team` points out of a
player to a team. `players: MEMBER <- Player[]` on `Team` is the same `MEMBER`
relationship seen from the other end, pointing in. The name before the arrow is
the relationship's type; both fields use `MEMBER`, so they are two views of one
relationship, and writing either one creates it.

Brackets after the target type mean "any number": `Player[]` is a list, and
`Team` without brackets is at most one.

## Properties on a relationship

A block after the target type in the schema declares properties that belong to
the relationship itself, such as the year a player joined. They are written and
read with `&`, so they cannot be mistaken for fields of either node. A property
without `?` must be given when the relationship is created.

```zql
mutation {
  Team(name: "Oilers") {
    players <- Player(name: "Alice" && position: "C") {
      &since: 2016
      &captain: true
    }
    players <- Player(name: "Bob" && position: "D") { &since: 2023 }
  }
}
```

```zql
query {
  Team(name: "Oilers") {
    players <- Player {
      name
      &since
      &captain
    }
  }
}
```

```json
{
  "players": [
    {
      "captain": true,
      "name": "Alice",
      "since": 2016
    },
    {
      "captain": null,
      "name": "Bob",
      "since": 2023
    }
  ]
}
```

Leaving out `&since`, which has no `?`, is an error:

```zql error
mutation {
  Team(name: "Flames") {
    players <- Player(name: "Dan" && position: "G")
  }
}
```

```text
execution error: error: players requires &since
  query:3:5
      players <- Player(name: "Dan" && position: "G")
      ^^^^^^^
  help: write `&since: …` inside Player
```

## Following a relationship

Follow a relationship from either end. From a player, `playsFor ->` leads to the
team; from the team, `players <-` leads back to its players. Related nodes can
have a filter of their own.

```zql
query {
  Team(name: "Oilers") {
    name
    players <- Player(position = "C") { name }
  }
}
```

```json
{
  "name": "Oilers",
  "players": [
    { "name": "Alice" }
  ]
}
```

```zql
query {
  Player(name: "Bob") {
    playsFor -> Team { name &since }
  }
}
```

```json
{ "playsFor": { "name": "Oilers", "since": 2023 } }
```

## Several hops

`*min..max` before the arrow follows the same relationship between `min` and
`max` times, and `@hops` says how many steps it took to reach each node. Here
Alice mentors Bob, who mentors Cara.

```zql
mutation {
  Player(name: "Cara" && position: "LW")
}
```

```zql
mutation {
  Player(name: "Bob") {
    mentors -> link Player(name: "Cara")
  }
}
```

```zql
mutation {
  Player(name: "Alice") {
    mentors -> link Player(name: "Bob")
  }
}
```

```zql
query {
  Player(name: "Alice") {
    mentors *1..3 -> Player { name @hops }
  }
}
```

```json
{
  "mentors": [
    { "hops": 1, "name": "Bob" },
    { "hops": 2, "name": "Cara" }
  ]
}
```

For the shortest or cheapest route between two nodes, with distances and
costs, see [paths](path.md).

## Walks in a filter

A filter can walk the graph as well as test fields. `has` follows a
relationship from the node being tested, and the parentheses after it test the
node it reaches. The schema knows every type, so a walk never names one. Cara
joins a second team first:

```zql
mutation {
  Team(name: "Flames")
}
```

```zql
mutation {
  Player(name: "Cara") {
    playsFor -> link Team(name: "Flames") { &since: 2024 }
  }
}
```

```zql
query {
  Player(has playsFor(name = "Oilers")) { name }
}
```

```json
[
  { "name": "Alice" },
  { "name": "Bob" }
]
```

A filter on a relationship in the braces filters only that list; every team
still comes back. In the parentheses, it filters the teams themselves:

```zql
query {
  Team {
    name
    players <- Player(position = "C") { name }
  }
}
```

```json
[
  {
    "name": "Oilers",
    "players": [
      { "name": "Alice" }
    ]
  },
  { "name": "Flames", "players": [] }
]
```

```zql
query {
  Team(has players(position = "C")) { name }
}
```

```json
[
  { "name": "Oilers" }
]
```

The whole grammar:

| form | meaning |
|---|---|
| `has rel` | follow a relationship; at least one related node must match |
| `!have rel` | no related node may match (`!has` is an error) |
| `in rel`, `with rel` | keep walking, one hop each; the two mean the same |
| `(…)` | test the node right before it |
| `same rel` | at the start of a walk: continue from the node `rel` reached earlier in this `&&` group |
| `in same rel` | at the end of a walk: arrive at that same node |
| `rel 2 hops`, `rel within 3 hops` | repeat one relationship exactly 2, or 1 to 3, times |

- **Any, and none.** A relationship to many holds when any related node
  matches; `!have` holds when none does. A node without the relationship has
  none of it.
- **No parentheses** means the relationship exists: `Player(has playsFor)` is
  every player on a team.
- Walks join with `&&` and `||` like any test, and never nest: the
  parentheses test one node's own fields, and a walk continues after them.

```zql
query {
  Team(!have players(position = "C")) { name }
}
```

```json
[
  { "name": "Flames" }
]
```

`in` and `with` take one more hop from the node before. Players who mentor
someone on the Oilers:

```zql
query {
  Player(has mentors in playsFor(name = "Oilers")) { name }
}
```

```json
[
  { "name": "Alice" }
]
```

### same

Two walks in one `&&` group are separate: `has mentors(position = "D") && has
mentors in playsFor(…)` may find two different players. `same mentors`
continues from the node the earlier walk reached, so both tests hold for one
player. At the end of a walk, `in same playsFor` has to arrive at the node an
earlier walk reached: a join, on the node itself rather than its fields.
Players who mentor a teammate:

```zql
query {
  Player(has playsFor && has mentors in same playsFor) { name }
}
```

```json
[
  { "name": "Alice" }
]
```

`same` names one node. It is an error after `||`, after `!have`, and when the
name is reached more than once before it.

### Several hops

`2 hops` repeats one relationship exactly twice, and `within 3 hops` one to
three times, up to 6. They are the words for `*2..2` and `*1..3`: a node counts
at its shortest distance, and a walk never goes back to a node it has passed.
Alice mentors Bob, who mentors Cara:

```zql
query {
  Player(has mentors 2 hops(name = "Cara")) { name }
}
```

```json
[
  { "name": "Alice" }
]
```

The same words work in the braces, where `mentors 2 hops -> Player` is
`mentors *2..2 -> Player`.

### Errors that show the walk

Testing a relationship as if it were a field is an error that writes the walk:

```zql error
query {
  Player(playsFor = "Oilers") { name }
}
```

```text
execution error: error: playsFor is a relationship
  query:2:10
    Player(playsFor = "Oilers") { name }
           ^^^^^^^^
  help: test a field of the related node: `has playsFor(name = "Oilers")`; Team has name
```

So are `playsFor.name = "Oilers"`, `playsFor -> Team(name = "Oilers")`,
`playsFor: Team(…)` and `!has`: each error shows the `has` or `!have` form.

### How it runs

When the test at the end of a walk can use an [index](index.md), the matching
nodes are found through the index first, and the walk is followed backwards
from them, so only the nodes that reach them are tested. When the node's own
fields are indexed, each candidate walks forward instead. `within N hops` to an
indexed end searches from both ends at once. Every relationship read counts
toward the query's time limit.

### On the Flights sample

The explorer's Flights sample stores routes as `route: ROUTE -> Airport[]`.
Here are five of its airports and the routes between them:

```zql
schema {
  type Country {
    name: String
    iso: String
    airports: BASE <- Airport[]
  }

  type Airport {
    code: String
    country: BASE -> Country
    route: ROUTE -> Airport[]
  }
}

unique {
  Airport { code }
  Country { iso }
}
```

```zql
mutation {
  Country(name: "Canada" && iso: "CA") {
    airports <- Airport(code: "YYC")
    airports <- Airport(code: "YYZ")
  }
}
```

```zql
mutation {
  Country(name: "Japan" && iso: "JP") {
    airports <- Airport(code: "NRT")
  }
}
```

```zql
mutation {
  Country(name: "United States" && iso: "US") {
    airports <- Airport(code: "ORD")
  }
}
```

```zql
mutation {
  Country(name: "Netherlands" && iso: "NL") {
    airports <- Airport(code: "AMS")
  }
}
```

```zql
mutation {
  Airport(code: "YYC") {
    route -> link Airport(code: "NRT")
    route -> link Airport(code: "ORD")
    route -> link Airport(code: "YYZ")
  }
}
```

```zql
mutation {
  Airport(code: "YYZ") {
    route -> link Airport(code: "AMS")
    route -> link Airport(code: "ORD")
  }
}
```

```zql
mutation {
  Airport(code: "NRT") {
    route -> link Airport(code: "AMS")
    route -> link Airport(code: "ORD")
  }
}
```

```zql
mutation {
  Airport(code: "ORD") {
    route -> link Airport(code: "AMS")
  }
}
```

Canadian airports with a route to Japan:

```zql
query {
  Airport(has country(iso = "CA") && has route in country(iso = "JP")) { code }
}
```

```json
[
  { "code": "YYC" }
]
```

Airports with no route to the United States:

```zql
query {
  Airport(!have route in country(iso = "US")) { code }
}
```

```json
[
  { "code": "ORD" },
  { "code": "AMS" }
]
```

Countries with an airport within two flights of Amsterdam:

```zql
query {
  Country(has airports with route within 2 hops(code = "AMS")) { name }
}
```

```json
[
  { "name": "Canada" },
  { "name": "Japan" },
  { "name": "United States" }
]
```

## Next

- [Mutations](mutation.md): creating and linking relationships.
- [Unique fields](unique.md).
