import { createDatabase } from 'zegadb';

export const dynamic = 'force-dynamic';

export async function GET() {
  const db = await createDatabase();
  try {
    return new Response(db.run('type Person { name: String }', 'mutation { Person(name: "Ada") { name } }'), {
      headers: { 'content-type': 'application/json' },
    });
  } finally { db.free(); }
}
