// The Worker's front door: CORS for explorer2, the admin-token check and the
// route to a size's Durable Object. No container code here, so the tests run
// it under plain Node with fake bindings (test/front.test.mjs).

const BINDINGS = { lite: 'ZEGA_LITE', basic: 'ZEGA_BASIC', std1: 'ZEGA_STD1' };

// explorer2.zega.dev calls this Worker from the browser; localhost is for
// running explorer2 locally (`npm run dev:remote` in browser/).
const ORIGINS = new Set(['https://explorer2.zega.dev']);
const LOCAL = /^http:\/\/(localhost|127\.0\.0\.1)(:\d{1,5})?$/;

export function json(body, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
}

function allowedOrigin(request) {
  const origin = request.headers.get('origin');
  return origin && (ORIGINS.has(origin) || LOCAL.test(origin)) ? origin : null;
}

// A preflight carries no Authorization header, so it is answered before the
// token check; it grants nothing but the right to send the real request.
function preflight(request) {
  const origin = allowedOrigin(request);
  if (!origin) return new Response(null, { status: 403, headers: { vary: 'Origin' } });
  return new Response(null, {
    status: 204,
    headers: {
      'access-control-allow-origin': origin,
      'access-control-allow-methods': 'GET, POST, DELETE',
      'access-control-allow-headers': 'authorization, content-type',
      // Chrome caps this at 2 h; without it every call would pay a second round trip.
      'access-control-max-age': '7200',
      vary: 'Origin',
    },
  });
}

function withCors(request, response) {
  const origin = allowedOrigin(request);
  const out = new Response(response.body, response);
  out.headers.append('vary', 'Origin');
  if (origin) out.headers.set('access-control-allow-origin', origin);
  return out;
}

async function authorized(request, env) {
  const header = request.headers.get('authorization') ?? '';
  const given = new TextEncoder().encode(header.startsWith('Bearer ') ? header.slice(7) : '');
  const expected = new TextEncoder().encode(env.ADMIN_TOKEN);
  if (given.byteLength !== expected.byteLength) return false;
  return crypto.subtle.timingSafeEqual(given, expected);
}

async function route(request, env) {
  if (!env.ADMIN_TOKEN) return json({ ok: false, error: 'ADMIN_TOKEN secret is not set' }, 500);
  if (!(await authorized(request, env))) return json({ ok: false, error: 'unauthorized' }, 401);
  const [prefix, size, graph] = new URL(request.url).pathname.split('/').filter(Boolean);
  const binding = env[BINDINGS[size]];
  if (prefix !== 'c' || !binding || !graph) {
    return json({ ok: false, error: 'use /c/<lite|basic|std1>/<graph>/bench/<kind> or /c/<size>/<graph>/zql' }, 404);
  }
  return binding.getByName(graph).fetch(request);
}

export async function handle(request, env) {
  if (request.method === 'OPTIONS') return preflight(request);
  return withCors(request, await route(request, env));
}
