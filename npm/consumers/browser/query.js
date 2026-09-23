import { createDatabase } from 'zegadb';

export async function run(options) {
  try {
    const db = await createDatabase(options);
    try {
      const result = JSON.parse(db.run('type Person { name: String }', 'mutation { Person(name: "Ada") { name } }'));
      if (JSON.stringify(result) !== '{"name":"Ada"}') throw new Error(`Wrong query result: ${JSON.stringify(result)}`);
      document.querySelector('#result').textContent = JSON.stringify(result);
      console.log(`browser: ${JSON.stringify(result)}`);
    } finally { db.free(); }
  } catch (error) {
    document.querySelector('#result').textContent = `ERROR: ${error}`;
    throw error;
  }
}
