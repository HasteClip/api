// The checks, run every minute from Cloudflare (so from outside HasteClip's
// server), and what's done with them: each part's checks are added up per
// hour, and its last one kept. Every fifth minute the server's numbers
// (/stats.json) and Uptime Kuma's incidents are fetched and kept, for when
// they can't be.
//
// Uptime Kuma, on HasteClip's server, checks the same things and tells
// Discord when one fails. When the whole server stops answering, Kuma can't,
// so that's told from here, and so is Kuma itself going quiet.

import type { Env } from './index';

/// How the checks introduce themselves. The server doesn't count their plays
/// of the test clip (src/pages.rs, is_status_check).
export const AGENT = 'HasteClip-Status/1 (+https://status.hasteclip.com)';
/// A clip shared for the checks (the API's src/testdata/clip.mp4).
const CLIP = 'Fjk5AiKEya';
export const STATS = 'https://clips.hasteclip.com/stats.json';
const KUMA = 'https://kuma.hasteclip.com/api/status-page/hasteclip';

export interface Part {
  id: string;
  name: string;
  group: string;
  /// What it's for, in a few words.
  about: string;
  url: string;
  init?: RequestInit;
  status: number[];
  /// Text the answer must have.
  contains?: string;
  /// Read the whole answer (to see the bytes arrive).
  read?: boolean;
}

export const PARTS: Part[] = [
  {
    id: 'pages',
    name: 'Clip pages',
    group: 'Watching clips',
    about: "Opening a clip's link",
    url: `https://clips.hasteclip.com/${CLIP}`,
    init: { method: 'HEAD' },
    status: [200],
  },
  {
    id: 'playback',
    name: 'Clip playback',
    group: 'Watching clips',
    about: 'Playing a clip, in a browser or a chat',
    url: `https://clips.hasteclip.com/${CLIP}.mp4`,
    init: { headers: { range: 'bytes=0-1023' } },
    status: [206],
    read: true,
  },
  {
    id: 'sharing',
    name: 'Sharing from the app',
    group: 'Sharing clips',
    about: 'Uploading a clip and getting its link',
    url: 'https://api.hasteclip.com/readyz',
    status: [200],
    contains: 'ok',
  },
  {
    id: 'website',
    name: 'Website',
    group: 'Website and app',
    about: 'hasteclip.com',
    url: 'https://hasteclip.com/',
    status: [200],
    contains: 'HasteClip',
  },
  {
    id: 'github',
    name: 'App updates and downloads',
    group: 'Website and app',
    about: "From GitHub, not HasteClip's server",
    url: 'https://github.com/hasteclip/hasteclip/releases/latest',
    status: [302],
  },
];

/// Uptime Kuma: not shown as a part, but whether it answers says whether
/// the server does, and its status page has the incidents.
const KUMA_PART: Part = { id: 'kuma', name: 'Uptime Kuma', group: '', about: '', url: KUMA, status: [200], contains: '"config"', read: true };

export interface Result {
  ok: boolean;
  ms: number;
  detail: string;
  /// Whether HasteClip's server answered at all: not a network error, and
  /// not Cloudflare saying it couldn't reach the server (52x).
  reached: boolean;
  body?: string;
}

export async function check(part: Part): Promise<Result> {
  const start = Date.now();
  try {
    const headers = { 'user-agent': AGENT, ...(part.init?.headers as Record<string, string> | undefined) };
    const res = await fetch(part.url, { ...part.init, headers, redirect: 'manual', signal: AbortSignal.timeout(10_000) });
    const body = part.read || part.contains ? await res.text() : (await res.body?.cancel(), '');
    // Workers' clock moves on I/O, so this is how long the answer took.
    const ms = Date.now() - start;
    const statusOk = part.status.includes(res.status);
    const textOk = !part.contains || body.includes(part.contains);
    const detail = statusOk && !textOk ? `${res.status}, without "${part.contains}"` : String(res.status);
    return { ok: statusOk && textOk, ms, detail, reached: res.status < 520 || res.status > 530, body };
  } catch (e) {
    return { ok: false, ms: Date.now() - start, detail: e instanceof Error ? e.message : String(e), reached: false };
  }
}

/// Every minute.
export async function run(env: Env, at: number): Promise<void> {
  const now = Math.floor(at / 1000);
  const minute = Math.floor(now / 60) % 60;
  const hour = Math.floor(now / 3600) * 3600;
  const all = [...PARTS, KUMA_PART];
  const results = await Promise.all(all.map(check));

  const stmts: D1PreparedStatement[] = [];
  all.forEach((part, i) => {
    const r = results[i];
    stmts.push(
      env.DB.prepare(
        `insert into checks (part, hour, ok, total, ms_sum) values (?1, ?2, ?3, 1, ?4)
         on conflict (part, hour) do update set ok = ok + excluded.ok, total = total + 1, ms_sum = ms_sum + excluded.ms_sum`,
      ).bind(part.id, hour, r.ok ? 1 : 0, r.ms),
      env.DB.prepare(
        `insert into state (part, ok, ms, checked_at, fails, since, detail) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         on conflict (part) do update set ok = excluded.ok, ms = excluded.ms, checked_at = excluded.checked_at,
           fails = case when excluded.ok = 1 then 0 else state.fails + 1 end,
           since = case when excluded.ok = 1 then null else coalesce(state.since, excluded.since) end,
           detail = excluded.detail`,
      ).bind(part.id, r.ok ? 1 : 0, r.ms, now, r.ok ? 0 : 1, r.ok ? null : now, r.detail),
    );
  });

  // Kuma's incidents, and the server's numbers, every fifth minute (or
  // whenever there are none yet).
  const kept = await env.DB.prepare('select key from snapshot').all<{ key: string }>();
  const have = new Set(kept.results.map((r) => r.key));
  const kuma = results[all.length - 1];
  if (kuma.ok && kuma.body && (minute % 5 === 0 || !have.has('kuma'))) {
    stmts.push(snapshot(env, 'kuma', kuma.body, now));
  }
  if (minute % 5 === 0 || !have.has('stats')) {
    const stats = await fetchStats();
    if (stats) stmts.push(snapshot(env, 'stats', stats, now));
  }
  // Checks older than 120 days aren't shown; once a day they go.
  if (minute === 0 && Math.floor(now / 3600) % 24 === 3) {
    stmts.push(env.DB.prepare('delete from checks where hour < ?1').bind(now - 120 * 86_400));
  }
  await env.DB.batch(stmts);

  // The whole server: neither Kuma nor the API answered (Cloudflare couldn't
  // reach them). A crashed API alone is Kuma's to tell.
  const byId = Object.fromEntries(all.map((p, i) => [p.id, results[i]]));
  const serverDown = !byId.kuma.reached && !byId.sharing.reached;
  await alert(env, now, 'server', serverDown, 3, {
    down: (min) =>
      `🔴 **HasteClip's server isn't answering** (from Cloudflare, for ${min} minutes): clip links, sharing and the website are down. ` +
      `Uptime Kuma runs on that server, so this comes from status.hasteclip.com.`,
    up: (min) => `🟢 **HasteClip's server is answering again**, after about ${min} minutes.`,
  });
  // Kuma alone: HasteClip's fine, but Kuma's own alerts won't come.
  await alert(env, now, 'kuma-alone', !byId.kuma.ok && !serverDown, 10, {
    down: (min) => `🟠 **Uptime Kuma isn't answering** (for ${min} minutes, ${byId.kuma.detail}). HasteClip is fine, but Kuma's alerts won't come until it's back.`,
    up: () => `🟢 **Uptime Kuma is answering again.**`,
  });
}

function snapshot(env: Env, key: string, body: string, now: number): D1PreparedStatement {
  return env.DB.prepare(
    `insert into snapshot (key, body, fetched_at) values (?1, ?2, ?3)
     on conflict (key) do update set body = excluded.body, fetched_at = excluded.fetched_at`,
  ).bind(key, body, now);
}

async function fetchStats(): Promise<string | null> {
  try {
    const res = await fetch(STATS, { headers: { 'user-agent': AGENT }, signal: AbortSignal.timeout(10_000) });
    if (!res.ok) return null;
    const body = await res.text();
    return JSON.parse(body).totals ? body : null;
  } catch {
    return null;
  }
}

/// Tell Discord once something's been down `after` checks in a row, and
/// again when it's back. Its state is a row in `state` of its own.
async function alert(
  env: Env,
  now: number,
  key: string,
  down: boolean,
  after: number,
  say: { down: (minutes: number) => string; up: (minutes: number) => string },
): Promise<void> {
  const prev = await env.DB.prepare('select fails, since, alerted from state where part = ?1')
    .bind(key)
    .first<{ fails: number; since: number | null; alerted: number }>();
  const fails = down ? (prev?.fails ?? 0) + 1 : 0;
  const since = down ? (prev?.since ?? now) : null;
  let alerted = prev?.alerted ?? 0;
  if (down && fails >= after && !alerted) {
    alerted = (await discord(env, say.down(fails))) ? 1 : 0;
  } else if (!down && alerted) {
    const minutes = Math.max(1, Math.round((now - (prev?.since ?? now)) / 60));
    if (await discord(env, say.up(minutes))) alerted = 0;
  }
  await env.DB.prepare(
    `insert into state (part, ok, checked_at, fails, since, alerted) values (?1, ?2, ?3, ?4, ?5, ?6)
     on conflict (part) do update set ok = excluded.ok, checked_at = excluded.checked_at, fails = excluded.fails,
       since = excluded.since, alerted = excluded.alerted`,
  )
    .bind(key, down ? 0 : 1, now, fails, since, alerted)
    .run();
}

async function discord(env: Env, content: string): Promise<boolean> {
  if (!env.DISCORD_WEBHOOK) return false;
  try {
    const res = await fetch(env.DISCORD_WEBHOOK, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ content, username: 'HasteClip status', allowed_mentions: { parse: [] } }),
    });
    return res.ok;
  } catch {
    return false;
  }
}
