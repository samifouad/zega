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

## Next

- [Mutations](mutation.md): creating and linking relationships.
- [Unique fields](unique.md).
