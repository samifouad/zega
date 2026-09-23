'use client';

import { useEffect, useState } from 'react';
import { createDatabase } from 'zega';

export default function Page() {
  const [result, setResult] = useState('Loading');
  useEffect(() => {
    createDatabase().then(db => {
      try {
        setResult(db.run('type Person { name: String }', 'mutation { Person(name: "Ada") { name } }'));
      } finally { db.free(); }
    }).catch(error => setResult(`ERROR: ${error}`));
  }, []);
  return <pre id="result">{result}</pre>;
}
