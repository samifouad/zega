export default {
  async fetch(request, env) {
    const asset = await env.ASSETS.fetch(request);
    const response = new Response(asset.body, asset);
    const pathname = new URL(request.url).pathname;

    if (asset.ok) {
      if (pathname.endsWith('.wasm')) {
        response.headers.set('Content-Type', 'application/wasm');
      } else if (/\.m?js$/.test(pathname)) {
        response.headers.set('Content-Type', 'text/javascript; charset=utf-8');
      }
    }
    response.headers.set('X-Content-Type-Options', 'nosniff');
    // Filenames are stable: revalidate JS and wasm together after an update.
    response.headers.set('Cache-Control', 'no-cache');
    // The wasm uses ordinary memory. COEP is unnecessary and would constrain
    // the existing Monaco CDN/data: workers and third-party sample images.
    return response;
  },
};
