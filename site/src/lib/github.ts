// Build-time GitHub star count with a graceful fallback.
// Fetched once per build (module-level cache dedupes multiple importers).
// If the repo is private/unreachable, returns null and the UI hides the count.
const REPO = 'onsails/right-agent';

let cache: Promise<number | null> | null = null;

export function getStars(): Promise<number | null> {
  if (!cache) {
    cache = fetch(`https://api.github.com/repos/${REPO}`, {
      headers: { Accept: 'application/vnd.github+json', 'User-Agent': 'right-agent-site' },
    })
      .then((r) => (r.ok ? r.json() : null))
      .then((d) => (d && typeof d.stargazers_count === 'number' ? d.stargazers_count : null))
      .catch(() => null);
  }
  return cache;
}

export function formatStars(n: number | null): string {
  if (n == null) return '';
  if (n >= 1000) {
    const k = n / 1000;
    return (k >= 10 ? Math.round(k).toString() : k.toFixed(1).replace(/\.0$/, '')) + 'k';
  }
  return String(n);
}
