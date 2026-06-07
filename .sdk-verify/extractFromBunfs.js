/* eslint-disable custom-rules/no-sync-fs */
// Extracts a file from Bun's $bunfs virtual filesystem to a real temp directory
// so it can be spawned as a subprocess (child processes cannot access $bunfs).
//
// Ships verbatim (unbundled) as the @anthropic-ai/claude-agent-sdk `./extract`
// export — keep it standalone and Node-importable (no Bun globals, no
// local-relative imports).

import { createHash } from 'crypto'
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  renameSync,
  unlinkSync,
  writeFileSync,
} from 'fs'
import { tmpdir as osTmpdir } from 'os'
import { basename, join } from 'path'

// Inlined from src/utils/tempfile.ts — this file ships verbatim (unbundled) as
// the @anthropic-ai/claude-agent-sdk/extract export, so it must have zero
// local-relative imports. Keep behavior in sync with tempfile.ts#tmpdir.
function tmpdir() {
  if (process.env.CLAUDE_CODE_TMPDIR) {
    return process.env.CLAUDE_CODE_TMPDIR
  }
  if (process.platform === 'darwin') {
    // eslint-disable-next-line custom-rules/no-hardcoded-tmp -- mirrors tempfile.ts: macOS /tmp works fine; os.tmpdir() below is for Android-on-Linux where /tmp isn't writable.
    return '/tmp'
  }
  return osTmpdir()
}

// Inlined from src/utils/tempfile.ts#claudeTempDir + verifyTempDirOwnership —
// keep behavior in sync. Per-UID Claude temp directory: `{tmpdir()}/claude-{uid}`
// on Unix, `{tmpdir()}/claude` on Windows (where %TEMP% is already per-user).
//
// Defense-in-depth against a local attacker pre-creating the extraction dir
// before the victim's first run: the content-hash dir name is fully predictable
// (sha256 of a public npm binary) under shared `/tmp`, and
// `mkdirSync({recursive:true})` is a silent no-op on a pre-existing dir without
// enforcing owner or mode. Without this check an attacker-owned directory would
// be used as-is, letting them swap the binary the SDK later spawns.
function safeTempBase() {
  const dir = join(
    tmpdir(),
    process.platform === 'win32'
      ? 'claude'
      : `claude-${process.getuid?.() ?? 0}`,
  )
  // Gate on getuid (POSIX-only) rather than process.platform: getuid presence
  // reflects the real OS. No-op on Windows where %TEMP% is already per-user.
  if (typeof process.getuid === 'function') {
    mkdirSync(dir, { recursive: true, mode: 0o700 })
    const uid = process.getuid()
    const st = lstatSync(dir)
    if (!st.isDirectory()) {
      throw new Error(
        `Temp directory ${dir} is not a directory (may be an attacker-planted symlink). Refusing to use it.`,
      )
    }
    if (st.uid !== uid) {
      throw new Error(
        `Temp directory ${dir} is owned by uid ${st.uid}, expected ${uid}. Refusing to use it — another user may have pre-created it.`,
      )
    }
    if ((st.mode & 0o777) !== 0o700) {
      // Owner matches, so this is our own dir from a prior run with a looser
      // umask rather than an attacker pre-create. Tighten it.
      chmodSync(dir, 0o700)
    }
  } else {
    // Windows: best-effort create; verification is skipped (%TEMP% is per-user).
    try {
      mkdirSync(dir, { recursive: true, mode: 0o700 })
    } catch {
      // best-effort; see comment above
    }
  }
  return dir
}

/**
 * If `embeddedPath` is inside Bun's $bunfs virtual filesystem, extract it to a
 * temp directory and return the real path. Otherwise return the path unchanged.
 *
 * Uses a content hash for the directory name so that:
 * - Same binary version reuses the same extracted file (no accumulation)
 * - Different versions get separate directories (no collision)
 * - Concurrent instances are safe (atomic write via temp file + rename)
 *
 * @param {string} embeddedPath — path returned by `import ... with { type: 'file' }`
 * @returns {string} — a real filesystem path that can be spawned as a subprocess
 */
export function extractFromBunfs(embeddedPath) {
  // Bun single-file embed: `/$bunfs/root/...` (POSIX) or `B:\~BUN\root\...`
  // (Windows — Bun's StandaloneModuleGraph uses `~BUN`, not `$bunfs`, on Windows).
  if (!embeddedPath.includes('$bunfs') && !embeddedPath.includes('~BUN')) {
    return embeddedPath
  }

  try {
    const content = readFileSync(embeddedPath)
    const hash = createHash('sha256').update(content).digest('hex').slice(0, 16)
    const tmpDir = join(safeTempBase(), `claude-agent-sdk-${hash}`)
    // Preserve the embedded file's basename — ProcessTransport.isNativeBinary()
    // decides whether to spawn directly or via node/bun based on extension, so
    // a native binary must not come out named cli.js.
    const fileName = basename(embeddedPath)
    const tmpPath = join(tmpDir, fileName)
    mkdirSync(tmpDir, { recursive: true, mode: 0o700 })
    // The directory is content-hash-keyed, so an existing tmpPath is byte-
    // identical to what we would write — short-circuit. This is a write
    // idempotency check on a content-addressed path, not a read TOCTOU
    // (the existence IS the contract). Without it, a second call (or a
    // concurrent app instance) on Windows would hit EPERM/EBUSY renaming over
    // the memory-mapped running .exe and fall back to the unspawnable $bunfs
    // path. It also skips a redundant write+chmod on every call.
    if (existsSync(tmpPath)) {
      return tmpPath
    }
    // Write to a temp file and atomically rename to avoid truncation races —
    // concurrent readers always see either the old complete file or the new one.
    const tmpFile = join(tmpDir, `${fileName}.tmp.${process.pid}`)
    writeFileSync(tmpFile, content)
    chmodSync(tmpFile, 0o755)
    try {
      renameSync(tmpFile, tmpPath)
    } catch (e) {
      if ((e.code === 'EPERM' || e.code === 'EBUSY') && existsSync(tmpPath)) {
        // Lost a concurrent first-time-extraction race on Windows: the winner's
        // child holds tmpPath memory-mapped, so the loser's rename over it fails.
        // The content-hash dir guarantees byte-identity, so the winner's file is
        // exactly what we'd have written — use it.
        try {
          unlinkSync(tmpFile)
        } catch {
          // best-effort cleanup of our orphaned .tmp.{pid} file
        }
        return tmpPath
      }
      throw e
    }
    return tmpPath
  } catch (err) {
    // biome-ignore lint/suspicious/noConsole: intentional user-facing warning in standalone SDK helper
    console.warn(
      `[claude-agent-sdk] Failed to extract CLI from $bunfs: ${err.message}. ` +
        `Child processes cannot access $bunfs paths — the CLI will likely fail to start.`,
    )
    return embeddedPath
  }
}
