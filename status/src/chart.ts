// Charts as SVG, drawn here: the page has no scripts. Each bar (or point)
// has a hit area the height of the chart with a <title>, which browsers show
// on hover; every value is also in the table under the chart.
//
// One series per chart, in one colour (--series), so no legend: the chart's
// heading says what it is. Bars at most 24px wide, rounded at the top only,
// 2px apart; lines 2px with a faint wash under them; hairline gridlines.

import { esc } from './format';

export interface Point {
  /// For the tooltip and the table: "24 Nov, 14:00".
  label: string;
  /// Under the axis, for the few points that get one.
  tick: string;
  value: number | null;
}

const W = 400;
const H = 150;
const LEFT = 40;
const RIGHT = 10;
const TOP = 12;
const BOTTOM = 22;
const PLOT_W = W - LEFT - RIGHT;
const PLOT_H = H - TOP - BOTTOM;
const BASE = TOP + PLOT_H;

/// A round top for the axis: 1, 2 or 5 times a power of ten, at least `max`.
/// Counts get an even top under ten, so the middle tick's a whole number.
function niceMax(max: number, whole = false): number {
  if (whole && max < 10) return Math.max(2, Math.ceil(max / 2) * 2);
  if (max <= 0) return 1;
  const p = 10 ** Math.floor(Math.log10(max));
  for (const m of [1, 2, 5, 10]) if (m * p >= max) return m * p;
  return 10 * p;
}

function axis(top: number, format: (n: number) => string): string {
  return [0, top / 2, top]
    .map((v) => {
      const y = BASE - (v / top) * PLOT_H;
      return `<line class="grid" x1="${LEFT}" x2="${W - RIGHT}" y1="${y}" y2="${y}"/><text class="tick" x="${LEFT - 6}" y="${y + 4}" text-anchor="end">${esc(format(v))}</text>`;
    })
    .join('');
}

/// Labels under the first, middle and last points.
function ticks(points: Point[], x: (i: number) => number): string {
  const n = points.length;
  const at = n < 3 ? [...points.keys()] : [0, Math.floor((n - 1) / 2), n - 1];
  return at
    .map((i, k) => {
      const anchor = at.length === 1 ? 'middle' : k === 0 ? 'start' : k === at.length - 1 ? 'end' : 'middle';
      const xx = anchor === 'start' ? x(i) - 6 : anchor === 'end' ? x(i) + 6 : x(i);
      return `<text class="tick" x="${xx}" y="${H - 6}" text-anchor="${anchor}">${esc(points[i].tick)}</text>`;
    })
    .join('');
}

function svg(title: string, inner: string): string {
  return `<svg class="chart" viewBox="0 0 ${W} ${H}" role="img" aria-label="${esc(title)}">${inner}</svg>`;
}

export function bars(title: string, points: Point[], format: (n: number) => string): string {
  const slot = PLOT_W / Math.max(points.length, 1);
  const width = Math.max(2, Math.min(24, slot - 2));
  const top = niceMax(Math.max(0, ...points.map((p) => p.value ?? 0)), true);
  const center = (i: number) => LEFT + slot * i + slot / 2;
  const marks = points
    .map((p, i) => {
      const v = p.value ?? 0;
      const h = (v / top) * PLOT_H;
      const x = center(i) - width / 2;
      const r = Math.min(4, width / 2, h);
      const bar =
        v > 0
          ? `<path class="bar" d="M${x},${BASE}V${BASE - h + r}Q${x},${BASE - h} ${x + r},${BASE - h}H${x + width - r}Q${x + width},${BASE - h} ${x + width},${BASE - h + r}V${BASE}Z"/>`
          : '';
      const hit = `<rect class="hit" x="${LEFT + slot * i}" y="${TOP}" width="${slot}" height="${PLOT_H}"/>`;
      return `<g class="mark"><title>${esc(`${p.label}: ${p.value === null ? 'no data' : format(v)}`)}</title>${hit}${bar}</g>`;
    })
    .join('');
  const base = `<line class="baseline" x1="${LEFT}" x2="${W - RIGHT}" y1="${BASE}" y2="${BASE}"/>`;
  return svg(title, axis(top, format) + base + marks + ticks(points, center));
}

export function line(title: string, points: Point[], format: (n: number) => string): string {
  const known = points.map((p) => p.value).filter((v): v is number => v !== null);
  const top = niceMax(Math.max(0, ...known));
  const step = PLOT_W / Math.max(points.length - 1, 1);
  const x = (i: number) => LEFT + step * i;
  const y = (v: number) => BASE - (v / top) * PLOT_H;
  // Runs of known points; a point with no data breaks the line.
  const runs: [number, number][][] = [];
  let run: [number, number][] = [];
  points.forEach((p, i) => {
    if (p.value === null) {
      if (run.length) runs.push(run);
      run = [];
    } else run.push([x(i), y(p.value)]);
  });
  if (run.length) runs.push(run);
  const paths = runs
    .map((r) => {
      const d = r.map(([px, py], k) => `${k ? 'L' : 'M'}${px.toFixed(1)},${py.toFixed(1)}`).join('');
      const area = r.length > 1 ? `<path class="area" d="${d}L${r[r.length - 1][0].toFixed(1)},${BASE}L${r[0][0].toFixed(1)},${BASE}Z"/>` : '';
      return `${area}<path class="line" d="${d}"/>`;
    })
    .join('');
  // The last value: a dot with a ring, and its value beside it.
  let end = '';
  for (let i = points.length - 1; i >= 0; i--) {
    const v = points[i].value;
    if (v === null) continue;
    const labelY = Math.max(TOP + 10, y(v) - 10);
    end = `<circle class="dot" cx="${x(i)}" cy="${y(v)}" r="4"/><text class="end" x="${x(i) - 6}" y="${labelY}" text-anchor="end">${esc(format(v))}</text>`;
    break;
  }
  const hits = points
    .map((p, i) => {
      const left = i === 0 ? LEFT : x(i) - step / 2;
      const right = i === points.length - 1 ? W - RIGHT : x(i) + step / 2;
      return `<g class="mark"><title>${esc(`${p.label}: ${p.value === null ? 'no data' : format(p.value)}`)}</title><rect class="hit" x="${left}" y="${TOP}" width="${right - left}" height="${PLOT_H}"/></g>`;
    })
    .join('');
  const base = `<line class="baseline" x1="${LEFT}" x2="${W - RIGHT}" y1="${BASE}" y2="${BASE}"/>`;
  return svg(title, axis(top, format) + base + paths + end + hits + ticks(points, x));
}

/// A part's last 30 days: a block a day, coloured by how many checks passed.
export interface Day {
  label: string;
  /// Checks that passed, of how many; null when it wasn't checked.
  ok: number;
  total: number;
}

export function uptime(name: string, days: Day[]): string {
  const w = 300;
  const h = 28;
  const gap = 2;
  const block = (w - gap * (days.length - 1)) / days.length;
  const blocks = days
    .map((d, i) => {
      const share = d.total ? d.ok / d.total : null;
      const kind = share === null ? 'none' : share >= 0.999 ? 'good' : share >= 0.98 ? 'warn' : 'bad';
      const text = share === null ? 'not checked' : `${percent(share)} of ${d.total.toLocaleString('en-US')} checks passed`;
      return `<rect class="day ${kind}" x="${(block + gap) * i}" y="0" width="${block}" height="${h}" rx="2"><title>${esc(`${d.label}: ${text}`)}</title></rect>`;
    })
    .join('');
  return `<svg class="days" viewBox="0 0 ${w} ${h}" preserveAspectRatio="none" role="img" aria-label="${esc(`${name}, the last 30 days`)}">${blocks}</svg>`;
}

export function percent(share: number): string {
  if (share >= 1) return '100%';
  const p = share * 100;
  return `${p >= 99 ? p.toFixed(2) : p.toFixed(1)}%`;
}
