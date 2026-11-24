// Text for the page: escaped, numbers in words people read, times in UTC
// (the page has no scripts to show them in the reader's own).

export function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

/// 1,284 under a hundred thousand, then 128K, 1.3M.
export function count(n: number): string {
  if (n < 100_000) return Math.round(n).toLocaleString('en-US');
  if (n < 1_000_000) return `${Math.round(n / 1000)}K`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

/// For axes: 0, 500, 1K, 1.5K.
export function short(n: number): string {
  if (n >= 1_000_000) return `${+(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${+(n / 1000).toFixed(1)}K`;
  return String(+n.toFixed(1));
}

/// As a computer's own file browser counts them: a GB is a billion bytes.
export function bytes(n: number): string {
  if (n >= 1e12) return `${(n / 1e12).toFixed(1)} TB`;
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${Math.round(n / 1e6)} MB`;
  return `${Math.round(n / 1e3)} KB`;
}

/// `37 hours`, or `45 minutes` under an hour.
export function duration(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? '' : 's'}`;
  const h = Math.round(minutes / 60);
  return `${count(h)} hour${h === 1 ? '' : 's'}`;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/// `24 Nov`.
export function day(seconds: number): string {
  const d = new Date(seconds * 1000);
  return `${d.getUTCDate()} ${MONTHS[d.getUTCMonth()]}`;
}

/// `14:00`.
export function clock(seconds: number): string {
  const d = new Date(seconds * 1000);
  return `${String(d.getUTCHours()).padStart(2, '0')}:${String(d.getUTCMinutes()).padStart(2, '0')}`;
}

/// `24 Nov, 14:00`.
export function when(seconds: number): string {
  return `${day(seconds)}, ${clock(seconds)}`;
}

/// `3 minutes ago`, `2 hours ago`.
export function ago(seconds: number, now: number): string {
  const m = Math.max(0, Math.round((now - seconds) / 60));
  if (m < 1) return 'just now';
  if (m < 60) return `${m} minute${m === 1 ? '' : 's'} ago`;
  const h = Math.round(m / 60);
  if (h < 48) return `${h} hour${h === 1 ? '' : 's'} ago`;
  return `${Math.round(h / 24)} days ago`;
}
