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

## Filtering by a related node

A relationship with its arrow and a target type can go inside a filter too. It
keeps the nodes that have at least one related node meeting the target's
condition. Cara joins a second team first:

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
  Player(playsFor -> Team(name = "Oilers")) { name }
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
  Team(players <- Player(position = "C")) { name }
}
```

```json
[
  { "name": "Oilers" }
]
```

- **Any, not all.** One matching related node is enough, for a relationship
  to one or to many. A node without the relationship does not match.
- **Direction.** The arrow is the one in the schema: `playsFor ->` from a
  player, `players <-` from a team.
- **No parentheses** on the target means the relationship exists:
  `Player(playsFor -> Team)` is every player on a team.
- **Nesting.** The target's condition can walk further, and joins with `&&` and
  `||` like any test. Players who mentor someone on the Oilers:

```zql
query {
  Player(mentors -> Player(playsFor -> Team(name = "Oilers"))) { name }
}
```

```json
[
  { "name": "Alice" }
]
```

A test on the related node goes through its relationship. Testing the
relationship as if it were a field is an error that shows the right form:

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
  help: filter by a field of the related node: `playsFor -> Team(name = "Oilers")`; Team has name
```

When the target's condition can use an [index](index.md), the matching targets
are found through the index first and the relationship is followed backwards
from them, so only the nodes that reach them are tested.

## Counting relationships

`@count(field)` is how many relationships a node has. It works in a filter,
with `=`, `!=`, `<`, `<=`, `>`, `>=` and a whole number; in the braces, where
its key is `count` unless you name it; and in `order by`.

```zql
query {
  Team(@count(players) >= 1) order by @count(players) desc {
    name
    size: @count(players)
  }
}
```

```json
[
  { "name": "Oilers", "size": 2 },
  { "name": "Flames", "size": 1 }
]
```

With an arrow and a target, it counts only the relationships whose related node
matches. `= 0` finds the nodes with none: here, players who mentor no defender.

```zql
query {
  Player(@count(mentors -> Player(position = "D")) = 0) { name }
}
```

```json
[
  { "name": "Bob" },
  { "name": "Cara" }
]
```

A node without the relationship counts 0. `@count` reads stored
relationships, so it is not allowed in a mutation's braces.

## Next

- [Mutations](mutation.md): creating and linking relationships.
- [Unique fields](unique.md).
