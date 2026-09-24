import { options, evict, zql, query } from './common.mjs';
const { endpoint, graph } = options();
try {
  await evict(endpoint, graph);
  const result = await zql(endpoint, graph, query('0'));
  if (result.coldStartMs === null) throw new Error('object did not cold start');
  console.log(JSON.stringify({ benchmark: 'cold', ok: true, endpoint, graph, ...result, value: undefined }));
} catch (error) { console.log(JSON.stringify({ benchmark: 'cold', ok: false, endpoint, graph, error: error.message })); process.exitCode = 1; }
