// What the page shows, from the checks kept in D1 and the last answers from
// the server (/stats.json) and Uptime Kuma.

import { PARTS, type Part } from './checks';
import type { Day, Point } from './chart';
import { clock, day, when } from './format';

export type Range = '24h' | '7d' | '30d';
export const RANGES: { id: Range; name: string; size: number; count: number }[] = [
  { id: '24h', name: '24 hours', size: 3600, count: 24 },
  { id: '7d', name: '7 days', size: 6 * 3600, count: 28 },
  { id: '30d', name: '30 days', size: 86_400, count: 30 },
];

export type Status = 'up' | 'down' | 'unknown';

export interface PartView {
  part: Part;
  status: Status;
  /// Its last answer, in milliseconds.
  ms: number | null;
  /// When it started failing.
  since: number | null;
  days: Day[];
  /// Of its checks in the last 30 days, how many passed.
  uptime: number | null;
}

/// What /stats.json answers (the API's src/status.rs).
export interface Stats {
  updated: string;
  totals: {
    clips: number;
    clips_ever: number;
    views: number;
    bytes: number;
    duration_ms: number;
    this_week: number;
    apps: number;
    counted_since: string | null;
  };
  games: { name: string; clips: number }[];
  hours: { hour: string; shares: number; opens: number; plays: number; sign_ups: number; clips: number; bytes: number }[];
  releases: { tag: string; published: string | null; downloads: number }[] | null;
}

export interface Incident {
  title: string;
  content: string;
  style: string;
  updated: string | null;
}

export interface View {
  now: number;
  range: (typeof RANGES)[number];
  parts: PartView[];
  serverDown: boolean;
  serverDownSince: number | null;
  incident: Incident | null;
  maintenance: { title: string; description: string }[];
  stats: Stats | null;
  statsAt: number | null;
  charts: { shares: Point[]; opens: Point[]; plays: Point[]; signUps: Point[]; stored: Point[]; response: Point[] };
}

interface StateRow {
  part: string;
  ok: number;
  ms: number | null;
  checked_at: number;
  fails: number;
  since: number | null;
}

/// A part counts as down after two failed checks in a row (one is often a
/// blip), and as unknown when it hasn't been checked for five minutes.
function status(row: StateRow | undefined, now: number): Status {
  if (!row || now - row.checked_at > 5 * 60) return 'unknown';
  return !row.ok && row.fails >= 2 ? 'down' : 'up';
}

export async function load(db: D1Database, rangeId: Range, now: number): Promise<View> {
  const range = RANGES.find((r) => r.id === rangeId) ?? RANGES[0];
  const today = Math.floor(now / 86_400) * 86_400;
  const from30 = today - 29 * 86_400;
  const end = Math.floor(now / range.size) * range.size;
  const start = end - (range.count - 1) * range.size;

  const [state, daily, response, snaps] = await db.batch<Record<string, unknown>>([
    db.prepare('select part, ok, ms, checked_at, fails, since from state'),
    db.prepare('select part, hour / 86400 * 86400 as day, sum(ok) as ok, sum(total) as total from checks where hour >= ?1 group by part, day').bind(from30),
    db.prepare("select hour, sum(ms_sum) as ms, sum(total) as n from checks where part = 'pages' and hour >= ?1 group by hour").bind(start),
    db.prepare('select key, body, fetched_at from snapshot'),
  ]);

  const states = new Map((state.results as unknown as StateRow[]).map((r) => [r.part, r]));
  const days = new Map<string, Map<number, { ok: number; total: number }>>();
  for (const r of daily.results as unknown as { part: string; day: number; ok: number; total: number }[]) {
    if (!days.has(r.part)) days.set(r.part, new Map());
    days.get(r.part)!.set(r.day, { ok: r.ok, total: r.total });
  }

  const parts = PARTS.map((part): PartView => {
    const row = states.get(part.id);
    const mine = days.get(part.id) ?? new Map();
    const list: Day[] = Array.from({ length: 30 }, (_, i) => {
      const d = from30 + i * 86_400;
      const n = mine.get(d);
      return { label: day(d), ok: n?.ok ?? 0, total: n?.total ?? 0 };
    });
    const total = list.reduce((s, d) => s + d.total, 0);
    const s = status(row, now);
    return {
      part,
      status: s,
      ms: row && s !== 'unknown' ? row.ms : null,
      since: s === 'down' ? (row?.since ?? null) : null,
      days: list,
      uptime: total ? list.reduce((s, d) => s + d.ok, 0) / total : null,
    };
  });

  const server = states.get('server');
  const serverDown = !!server && !server.ok && server.fails >= 2 && now - server.checked_at <= 5 * 60;

  const snap = new Map((snaps.results as unknown as { key: string; body: string; fetched_at: number }[]).map((r) => [r.key, r]));
  let stats: Stats | null = null;
  try {
    stats = snap.has('stats') ? (JSON.parse(snap.get('stats')!.body) as Stats) : null;
  } catch {
    stats = null;
  }
  let incident: Incident | null = null;
  let maintenance: View['maintenance'] = [];
  try {
    const kuma = snap.has('kuma') ? JSON.parse(snap.get('kuma')!.body) : null;
    if (kuma?.incident) {
      incident = {
        title: String(kuma.incident.title ?? ''),
        content: String(kuma.incident.content ?? ''),
        style: String(kuma.incident.style ?? 'info'),
        updated: kuma.incident.lastUpdatedDate ?? kuma.incident.createdDate ?? null,
      };
    }
    maintenance = (kuma?.maintenanceList ?? []).map((m: { title?: string; description?: string }) => ({
      title: String(m.title ?? 'Maintenance'),
      description: String(m.description ?? ''),
    }));
  } catch {
    // Kuma's answer wasn't what's expected: show no incident.
  }

  // The range's buckets, oldest first, and a label for each.
  const buckets = Array.from({ length: range.count }, (_, i) => start + i * range.size);
  const label = (t: number) => (range.size >= 86_400 ? day(t) : range.size > 3600 ? `${when(t)}–${clock(t + range.size)}` : when(t));
  const tick = (t: number) => (range.size >= 86_400 ? day(t) : range.id === '24h' ? clock(t) : day(t));
  const index = (t: number) => Math.floor((t - start) / range.size);

  const sums = { shares: new Array(range.count).fill(0), opens: new Array(range.count).fill(0), plays: new Array(range.count).fill(0), signUps: new Array(range.count).fill(0) };
  const stored: (number | null)[] = new Array(range.count).fill(null);
  const seen: boolean[] = new Array(range.count).fill(false);
  for (const h of stats?.hours ?? []) {
    const i = index(Date.parse(h.hour) / 1000);
    if (i < 0 || i >= range.count) continue;
    sums.shares[i] += h.shares;
    sums.opens[i] += h.opens;
    sums.plays[i] += h.plays;
    sums.signUps[i] += h.sign_ups;
    stored[i] = h.bytes; // hours come oldest first, so this ends as the bucket's last
    seen[i] = true;
  }
  // Before the first hour the server counted, there's no data, not zero.
  const first = seen.indexOf(true);
  const counts = (values: number[]): Point[] =>
    buckets.map((t, i) => ({ label: label(t), tick: tick(t), value: first === -1 || i < first ? null : values[i] }));
  // What's stored stays stored through an hour with nothing counted.
  for (let i = 1; i < stored.length; i++) if (stored[i] === null && first !== -1 && i > first) stored[i] = stored[i - 1];

  const ms = new Array(range.count).fill(0);
  const n = new Array(range.count).fill(0);
  for (const r of response.results as unknown as { hour: number; ms: number; n: number }[]) {
    const i = index(r.hour);
    if (i < 0 || i >= range.count) continue;
    ms[i] += r.ms;
    n[i] += r.n;
  }

  return {
    now,
    range,
    parts,
    serverDown,
    serverDownSince: serverDown ? (server?.since ?? null) : null,
    incident,
    maintenance,
    stats,
    statsAt: snap.get('stats')?.fetched_at ?? null,
    charts: {
      shares: counts(sums.shares),
      opens: counts(sums.opens),
      plays: counts(sums.plays),
      signUps: counts(sums.signUps),
      stored: buckets.map((t, i) => ({ label: label(t), tick: tick(t), value: stored[i] })),
      response: buckets.map((t, i) => ({ label: label(t), tick: tick(t), value: n[i] ? Math.round(ms[i] / n[i]) : null })),
    },
  };
}
