import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const siteRoot = dirname(scriptDir);
const distRoot = join(siteRoot, 'dist');

const expectEnabled = process.argv.includes('--expect-enabled');
const expectDisabled = process.argv.includes('--expect-disabled');

if (expectEnabled === expectDisabled) {
  console.error('Pass exactly one mode: --expect-enabled or --expect-disabled');
  process.exit(2);
}

const expectedKey = process.env.PUBLIC_POSTHOG_KEY || 'phc_posthog_test_key';
// Host is hardcoded (reverse proxy) in src/components/PostHog.astro — keep in sync.
const expectedHost = 'https://f.right-agent.ai';

const pages = [
  ['landing', join(distRoot, 'index.html')],
  ['docs', join(distRoot, 'docs', 'index.html')],
];

function assert(condition, message) {
  if (!condition) {
    console.error(message);
    process.exitCode = 1;
  }
}

function countOccurrences(haystack, needle) {
  return haystack.split(needle).length - 1;
}

function assertOccurrenceCount(name, html, needle, expectedCount) {
  const actualCount = countOccurrences(html, needle);
  assert(
    actualCount === expectedCount,
    `${name}: expected ${expectedCount} occurrence(s) of ${needle}, found ${actualCount}`,
  );
}

for (const [name, path] of pages) {
  assert(existsSync(path), `${name}: missing built file at ${path}`);
  if (!existsSync(path)) continue;

  const html = readFileSync(path, 'utf8');

  if (expectDisabled) {
    const absent = [
      'window.posthog',
      'posthog.init',
      'data-posthog-key=',
    ];

    for (const needle of absent) {
      assertOccurrenceCount(name, html, needle, 0);
    }

    continue;
  }

  const singletons = [
    'window.posthog=e',
    'posthog.init',
    `data-posthog-key="${expectedKey}"`,
    `data-posthog-host="${expectedHost}"`,
  ];

  for (const needle of singletons) {
    assertOccurrenceCount(name, html, needle, 1);
  }

  const required = [
    "defaults: '2026-01-30'",
    "capture_pageview: 'history_change'",
    'autocapture: true',
    'disable_session_recording: true',
  ];

  for (const needle of required) {
    assert(html.includes(needle), `${name}: missing ${needle}`);
  }
}

if (process.exitCode) {
  process.exit(process.exitCode);
}
