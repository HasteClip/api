// status.hasteclip.com: a Cloudflare Worker, so the page answers even when
// HasteClip's server doesn't. Every minute it checks each part from
// Cloudflare (checks.ts); the page (page.ts) shows those checks with the
// numbers the server last gave (data.ts). Its files (the font, the glyph,
// the stylesheet) are in public/.

import { run } from './checks';
import { load, type Range } from './data';
import { render } from './page';

export interface Env {
  DB: D1Database;
  /// Told when the whole server stops answering, or Uptime Kuma does.
  DISCORD_WEBHOOK?: string;
}

const HEADERS = {
  'content-type': 'text/html; charset=utf-8',
  'cache-control': 'public, max-age=30',
  'content-security-policy':
    "default-src 'none'; style-src 'self'; font-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
  'referrer-policy': 'no-referrer',
  'x-content-type-options': 'nosniff',
};

export default {
  async fetch(req, env, ctx): Promise<Response> {
    const url = new URL(req.url);
    if (url.pathname !== '/') return new Response('Not found', { status: 404 });
    if (req.method !== 'GET' && req.method !== 'HEAD') return new Response(null, { status: 405, headers: { allow: 'GET, HEAD' } });
    const range = (['24h', '7d', '30d'] as const).find((r) => r === url.searchParams.get('range')) ?? '24h';

    // The page is the same for everyone for half a minute.
    const key = new Request(`${url.origin}/?range=${range}`);
    const cached = await caches.default.match(key);
    if (cached) return req.method === 'HEAD' ? new Response(null, cached) : cached;
    const view = await load(env.DB, range as Range, Math.floor(Date.now() / 1000));
    const res = new Response(render(view), { headers: HEADERS });
    ctx.waitUntil(caches.default.put(key, res.clone()));
    return req.method === 'HEAD' ? new Response(null, res) : res;
  },

  async scheduled(event, env, ctx): Promise<void> {
    ctx.waitUntil(run(env, event.scheduledTime));
  },
} satisfies ExportedHandler<Env>;
