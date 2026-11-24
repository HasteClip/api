// The page: whether HasteClip works, each part's last 30 days, and the
// numbers and charts from the server. Plain HTML, no scripts, in the share
// pages' look (../static/style.css's tokens).

import { bars, line, percent, uptime, type Point } from './chart';
import { RANGES, type PartView, type View } from './data';
import { ago, bytes, clock, count, day, duration, esc, short, when } from './format';

export const STYLE_VERSION = '1';

const STATUS_TEXT = { up: 'Working', down: 'Not working', unknown: 'Not checked lately' } as const;
const STATUS_ICON = { up: '✓', down: '✕', unknown: '?' } as const;

function verdict(v: View): { kind: string; icon: string; title: string; detail: string } {
  if (v.serverDown) {
    const since = v.serverDownSince ? ` since ${clock(v.serverDownSince)} UTC` : '';
    return {
      kind: 'bad',
      icon: '✕',
      title: "HasteClip's server isn't answering",
      detail: `Clip links, sharing and the website have been down${since}.`,
    };
  }
  const down = v.parts.filter((p) => p.status === 'down');
  if (down.length) {
    return {
      kind: 'bad',
      icon: '✕',
      title: down.length === 1 ? `${down[0].part.name}: not working` : 'Some of HasteClip isn’t working',
      detail: `${down.map((p) => p.part.name).join(', ')}. The rest is working.`,
    };
  }
  if (v.parts.some((p) => p.status === 'unknown')) {
    return { kind: 'none', icon: '?', title: 'Not checked lately', detail: "The checks haven't run in the last few minutes." };
  }
  return { kind: 'good', icon: '✓', title: "Everything's working", detail: 'Every part answered its last check.' };
}

function partRow(p: PartView): string {
  const ms = p.ms !== null && p.status === 'up' ? `<span class="ms">${count(p.ms)} ms</span>` : '';
  const since = p.since ? `<span class="ms">since ${clock(p.since)} UTC</span>` : '';
  return `<li class="part">
    <div class="part-head">
      <div><strong>${esc(p.part.name)}</strong><span class="about">${esc(p.part.about)}</span></div>
      <div class="part-state"><span class="pill ${p.status}"><span aria-hidden="true">${STATUS_ICON[p.status]}</span> ${STATUS_TEXT[p.status]}</span>${ms}${since}</div>
    </div>
    ${uptime(p.part.name, p.days)}
    <div class="days-foot"><span>${esc(p.days[0].label)}</span><span>${p.uptime === null ? 'Not checked yet' : `${percent(p.uptime)} of checks passed`}</span><span>Today</span></div>
  </li>`;
}

function table(points: Point[], name: string, format: (n: number) => string): string {
  const rows = points
    .filter((p) => p.value !== null)
    .map((p) => `<tr><td>${esc(p.label)}</td><td>${esc(format(p.value!))}</td></tr>`)
    .reverse()
    .join('');
  return `<details><summary>As a table</summary><table><thead><tr><th>${points.length && points[0].label.includes(':') ? 'From (UTC)' : 'Day (UTC)'}</th><th>${esc(name)}</th></tr></thead><tbody>${rows || '<tr><td colspan="2">Nothing counted yet</td></tr>'}</tbody></table></details>`;
}

function chartCard(title: string, sub: string, chart: string, points: Point[], name: string, format: (n: number) => string): string {
  return `<section class="card chart-card"><h3>${esc(title)}</h3><p class="sub">${esc(sub)}</p>${chart}${table(points, name, format)}</section>`;
}

const total = (points: Point[]) => points.reduce((s, p) => s + (p.value ?? 0), 0);

function charts(v: View): string {
  const c = v.charts;
  const span = `in the last ${v.range.name}`;
  const n = (x: number) => count(x);
  const lastStored = [...c.stored].reverse().find((p) => p.value !== null)?.value ?? null;
  const known = c.response.filter((p) => p.value !== null);
  const avgMs = known.length ? Math.round(total(known) / known.length) : null;
  const per = v.range.size >= 86_400 ? 'a day' : v.range.size > 3600 ? 'every 6 hours' : 'an hour';
  return [
    chartCard('Clips shared', `${n(total(c.shares))} ${span}, ${per}`, bars('Clips shared', c.shares, short), c.shares, 'Clips shared', n),
    chartCard('Clips opened', `${n(total(c.opens))} ${span}: a clip's page opened`, bars('Clips opened', c.opens, short), c.opens, 'Opened', n),
    chartCard('Videos played', `${n(total(c.plays))} ${span}, in browsers and chats`, bars('Videos played', c.plays, short), c.plays, 'Played', n),
    chartCard('New apps sharing', `${n(total(c.signUps))} ${span}: apps sharing for the first time`, bars('New apps sharing', c.signUps, short), c.signUps, 'New apps', n),
    chartCard('Stored', lastStored === null ? 'Not counted yet' : `${bytes(lastStored)} of clips now`, line('Stored', c.stored, (x) => (x ? (x >= 1e9 ? `${short(x / 1e9)} GB` : bytes(x)) : '0')), c.stored, 'Stored', bytes),
    chartCard('Response time', avgMs === null ? 'Not checked yet' : `${avgMs} ms on average: opening a clip, from Cloudflare`, line('Response time', c.response, (x) => `${short(x)} ms`), c.response, 'Response time', (x) => `${x} ms`),
  ].join('');
}

function numbers(v: View): string {
  const s = v.stats;
  if (!s) return `<p class="muted">The numbers show here once the server has answered.</p>`;
  const t = s.totals;
  const downloads = s.releases ? s.releases.reduce((n, r) => n + r.downloads, 0) : null;
  const since = t.counted_since ? day(Date.parse(t.counted_since) / 1000) : null;
  const tile = (value: string, label: string, note = '') =>
    `<div class="tile"><span class="value">${esc(value)}</span><span class="label">${esc(label)}</span>${note ? `<span class="note">${esc(note)}</span>` : ''}</div>`;
  return `<div class="tiles">
    ${tile(count(t.clips), 'clips shared now')}
    ${tile(count(t.clips_ever), 'clips shared in all', since ? `counted since ${since}` : '')}
    ${tile(count(t.this_week), 'shared in the last 7 days')}
    ${tile(count(t.views), 'times a clip was opened')}
    ${tile(duration(t.duration_ms), 'of video')}
    ${tile(bytes(t.bytes), 'stored')}
    ${downloads === null ? '' : tile(count(downloads), 'downloads of the app', 'from GitHub, not counting updates')}
    ${tile(count(t.apps), 'apps have shared')}
  </div>`;
}

function lists(v: View): string {
  const s = v.stats;
  if (!s) return '';
  const games = s.games.length
    ? `<section class="card"><h3>Most shared games</h3><ol class="rows">${s.games
        .map((g) => `<li><span>${esc(g.name)}</span><span class="num">${count(g.clips)} clips</span></li>`)
        .join('')}</ol><p class="sub">Clips that are up, of games at least three apps have shared.</p></section>`
    : '';
  const releases = s.releases?.length
    ? `<section class="card"><h3>Downloads by version</h3><ol class="rows">${s.releases
        .slice(0, 6)
        .map(
          (r) =>
            `<li><span>${esc(r.tag)}${r.published ? ` <span class="muted">${esc(day(Date.parse(r.published) / 1000))}</span>` : ''}</span><span class="num">${count(r.downloads)}</span></li>`,
        )
        .join('')}</ol><p class="sub">Installers downloaded from GitHub, not counting updates.</p></section>`
    : '';
  return games || releases ? `<div class="pair">${games}${releases}</div>` : '';
}

/// Kuma's times are UTC, as "2026-11-24 09:40:00".
function kumaTime(s: string): string {
  const t = Date.parse(`${s.replace(' ', 'T')}Z`);
  return Number.isNaN(t) ? s : when(t / 1000);
}

export function render(v: View): string {
  const verdictNow = verdict(v);
  const groups = [...new Set(v.parts.map((p) => p.part.group))];
  const incident = v.incident
    ? `<section class="notice ${esc(v.incident.style)}"><h2>${esc(v.incident.title)}</h2><p>${esc(v.incident.content).replace(/\n/g, '<br>')}</p>${
        v.incident.updated ? `<p class="muted">Updated ${esc(kumaTime(v.incident.updated))} UTC</p>` : ''
      }</section>`
    : '';
  const maintenance = v.maintenance
    .map((m) => `<section class="notice info"><h2>${esc(m.title)}</h2>${m.description ? `<p>${esc(m.description)}</p>` : ''}</section>`)
    .join('');
  const tabs = RANGES.map(
    (r) => `<a href="?range=${r.id}#activity"${r.id === v.range.id ? ' aria-current="page"' : ''}>${esc(r.name)}</a>`,
  ).join('');
  const statsNote = v.statsAt
    ? v.serverDown
      ? `The last numbers the server gave, ${ago(v.statsAt, v.now)}.`
      : `From the server, ${ago(v.statsAt, v.now)}.`
    : '';

  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="theme-color" content="#111215">
<title>HasteClip status</title>
<meta name="description" content="Whether HasteClip's share links work, and how much they're used.">
<link rel="stylesheet" href="/static/style.css?v=${STYLE_VERSION}">
<link rel="icon" href="/static/glyph.svg" type="image/svg+xml">
</head>
<body>
<header class="bar">
  <a class="brand" href="https://hasteclip.com"><img src="/static/glyph.svg" alt="">HasteClip</a>
  <a class="button" href="https://hasteclip.com/download">Get HasteClip</a>
</header>
<main>
  <h1>Status</h1>
  <section class="verdict ${verdictNow.kind}">
    <span class="verdict-icon" aria-hidden="true">${verdictNow.icon}</span>
    <div><h2>${esc(verdictNow.title)}</h2><p>${esc(verdictNow.detail)}</p></div>
  </section>
  ${incident}${maintenance}

  ${groups
    .map(
      (g) => `<section class="group"><h2>${esc(g)}</h2><ul class="parts card">${v.parts
        .filter((p) => p.part.group === g)
        .map(partRow)
        .join('')}</ul></section>`,
    )
    .join('')}
  <p class="legend"><span class="key good"></span>All checks passed <span class="key warn"></span>Some failed <span class="key bad"></span>Down for a while <span class="key none"></span>Not checked</p>

  <section class="group">
    <h2>HasteClip in numbers</h2>
    ${numbers(v)}
  </section>

  <section class="group" id="activity">
    <div class="group-head"><h2>Activity</h2><nav class="tabs" aria-label="Time range">${tabs}</nav></div>
    <div class="charts">${charts(v)}</div>
    ${lists(v)}
  </section>

  <p class="foot-note">Checked every minute from Cloudflare's network, outside HasteClip's server. Times are UTC. ${esc(statsNote)}
  The numbers are totals: nothing here is about anyone in particular, and the checks' own visits aren't counted.</p>
</main>
<footer>
  <a href="https://hasteclip.com">HasteClip</a> · a fast, free clip recorder for Windows, macOS and Linux · <a href="https://github.com/hasteclip/api">Source</a>
</footer>
</body>
</html>`;
}

