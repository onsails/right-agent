// GitHub repo star count for the hero badge.
// Fetched client-side (see Hero.astro) so it stays current between deploys —
// no build-time API call, nothing to go stale.
export const REPO = 'onsails/right-agent';

export function formatStars(n: number | null): string {
  if (n == null) return '';
  if (n >= 1000) {
    const k = n / 1000;
    return (k >= 10 ? Math.round(k).toString() : k.toFixed(1).replace(/\.0$/, '')) + 'k';
  }
  return String(n);
}
