// The classic example graph, in ZQL. Loaded statement-by-statement by the
// "load sample graph" button.
export const SAMPLE_STATEMENTS = [
  `CREATE (p:Person {name: "Tom Hanks", born: 1956})`,
  `CREATE (p:Person {name: "Meg Ryan", born: 1961})`,
  `CREATE (p:Person {name: "Robert Zemeckis", born: 1952})`,
  `CREATE (m:Movie {title: "Apollo 13", released: 1995, tagline: "Houston, we have a problem."})`,
  `CREATE (m:Movie {title: "You've Got Mail", released: 1998})`,
  `CREATE (m:Movie {title: "Sleepless in Seattle", released: 1993})`,
  `CREATE (m:Movie {title: "Cast Away", released: 2000})`,
  `CREATE (m:Movie {title: "Forrest Gump", released: 1994, tagline: "Life is like a box of chocolates."})`,
  `MATCH (a {name: "Tom Hanks"}), (m {title: "Apollo 13"}) CREATE (a)-[:ACTED_IN {role: "Jim Lovell"}]->(m)`,
  `MATCH (a {name: "Tom Hanks"}), (m {title: "Forrest Gump"}) CREATE (a)-[:ACTED_IN {role: "Forrest Gump"}]->(m)`,
  `MATCH (a {name: "Tom Hanks"}), (m {title: "Cast Away"}) CREATE (a)-[:ACTED_IN {role: "Chuck Noland"}]->(m)`,
  `MATCH (a {name: "Meg Ryan"}), (m {title: "You've Got Mail"}) CREATE (a)-[:ACTED_IN {role: "Kathleen Kelly"}]->(m)`,
  `MATCH (a {name: "Meg Ryan"}), (m {title: "Sleepless in Seattle"}) CREATE (a)-[:ACTED_IN {role: "Annie Reed"}]->(m)`,
  `MATCH (a {name: "Robert Zemeckis"}), (m {title: "Apollo 13"}) CREATE (a)-[:DIRECTED]->(m)`,
  `MATCH (a {name: "Robert Zemeckis"}), (m {title: "Cast Away"}) CREATE (a)-[:DIRECTED]->(m)`,
  `MATCH (a {name: "Robert Zemeckis"}), (m {title: "Forrest Gump"}) CREATE (a)-[:DIRECTED]->(m)`,
  `MATCH (a {name: "Tom Hanks"}), (b {name: "Meg Ryan"}) CREATE (a)-[:KNOWS]->(b)`,
];
