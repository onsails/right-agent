# Turso multiprocess WAL production risk — Right/Riskoff

**Date:** 2026-08-24  
**Scope:** Turso multiprocess WAL short-read risk for Right/Riskoff production use.

## Decision

Do not use Turso multiprocess WAL with critical Right/Riskoff data. Our v0.7.2 fixture still reproduces the short-read failure at offset **16512**. Short term, avoid concurrent `TRUNCATE` checkpoints. Long term, move to a single-owner DB service with IPC. Recover Riskoff offline.

## What maintainers explicitly say

- The official manual says the feature is **“not production ready”** and **“do not use it for critical data right now”** ([manual](https://github.com/tursodatabase/turso/blob/main/docs/manual.md)).
- Maintainer @penberg closed #769 with **“Fixed by commit 7cf1d01d0”** ([#769](https://github.com/tursodatabase/turso/issues/769)). That commit calls the work **“initial support”** and says **“More work is still needed in the entire commit/checkpoint/recovery path”** ([7cf1d01d0](https://github.com/tursodatabase/turso/commit/7cf1d01d0)).

## Closest open issues ranked with reporter-vs-maintainer status


| Rank | Issue | Reporter/maintainer status and closeness |
|---|---|---|
| 1 | [#8348](https://github.com/tursodatabase/turso/issues/8348) | Closest reporter-only match: macOS arm64, v0.7.1/v0.7.2, multiprocess WAL, concurrent `TRUNCATE`, then a WAL-frame short read and abort. Its offset is **107152**, not Right’s **16512**. No maintainer response appears on the issue. |
| 2 | [#7833](https://github.com/tursodatabase/turso/issues/7833) | Reporter-authored ignored test labeled `known bug: a cross-process DB-file reader does not block a truncate checkpoint`. No maintainer response or linked fix appears on the issue. |
| 3 | [#8195](https://github.com/tursodatabase/turso/issues/8195) | Turso’s triage bot reproduced the WAL-frame lookup panic; @penberg requested a fix. No linked fix appears on the issue. |
| 4 | [#7213](https://github.com/tursodatabase/turso/issues/7213) | Reporter reproduced a multiprocess reader-slot ownership panic. Maintainer response: “Thanks for the detailed bug report.” No linked fix. |
| — | [#7340](https://github.com/tursodatabase/turso/issues/7340) | Distinct second-process WAL-lock bug. Merged [#7809](https://github.com/tursodatabase/turso/pull/7809) fixes that open path, not the short-read/checkpoint path. |

No maintainer has confirmed Right/Riskoff’s exact offset-16512 cause.

## Released fixes/version facts

- #769 closed the general access request after initial multiprocess support landed; it did not certify checkpoint/recovery behavior.
- [#7809](https://github.com/tursodatabase/turso/pull/7809) fixes the distinct second-process WAL-lock failure.
- No linked released fix appears for the short-read/checkpoint family in [#8348](https://github.com/tursodatabase/turso/issues/8348), [#7833](https://github.com/tursodatabase/turso/issues/7833), or [#8195](https://github.com/tursodatabase/turso/issues/8195).
- The [v0.7.2-tagged changelog](https://github.com/tursodatabase/turso/blob/v0.7.2/CHANGELOG.md) is cumulative under a `0.7.0` heading. Its adjacent WAL fixes cannot be attributed to the v0.7.1→v0.7.2 delta or to this failure without commit-level evidence.
- Fact: our tested v0.7.2 still reproduces the exact fixture at offset **16512**.

## Application to Right/Riskoff


- Fact: Right’s failure is offset **16512**; the closest known open issue is offset **107152**, not exact.
- Fact: the official manual says the feature is not production ready and should not be used with critical data.
- Inference: Riskoff must not rely on Turso multiprocess WAL for critical data.
- Fact: our v0.7.2 fixture reproduces the exact Right failure. No reviewed upstream source identifies a released fix for it.
- Inference: production exposure should be reduced by using a single-owner DB service/IPC and avoiding concurrent `TRUNCATE`/`checkpoint`.

## Recommended action

1. Do not delete `data.db-wal` or `data.db-tshm` as a live recovery action; upstream does not prescribe this.
2. Recover Riskoff offline from verified raw copies and produce a validated clean snapshot.
3. Short term, prevent concurrent `TRUNCATE` checkpoints while peer processes hold the database.
4. Long term, move Right database ownership to one process and route bot/aggregator access through IPC.
5. File a minimal upstream Rust issue containing the offset-16512 fixture and link [#8348](https://github.com/tursodatabase/turso/issues/8348), [#7833](https://github.com/tursodatabase/turso/issues/7833), and [#8195](https://github.com/tursodatabase/turso/issues/8195). Do not claim maintainers confirmed Right’s exact cause.
6. Reassess only after a maintainer-linked fix reaches a release and the fixture passes.

## Primary sources

- [Official manual](https://github.com/tursodatabase/turso/blob/main/docs/manual.md)
- [Official multiprocess access documentation](https://github.com/tursodatabase/turso/blob/main/docs/sql-reference/multiprocess-access.mdx)
- [Initial multiprocess implementation, 7cf1d01d0](https://github.com/tursodatabase/turso/commit/7cf1d01d0)
- [#769](https://github.com/tursodatabase/turso/issues/769)
- [#8348](https://github.com/tursodatabase/turso/issues/8348)
- [#7833](https://github.com/tursodatabase/turso/issues/7833)
- [#7213](https://github.com/tursodatabase/turso/issues/7213)
- [#7340](https://github.com/tursodatabase/turso/issues/7340)
- [#7809](https://github.com/tursodatabase/turso/pull/7809)
- [#8195](https://github.com/tursodatabase/turso/issues/8195)
- [v0.7.2-tagged cumulative changelog](https://github.com/tursodatabase/turso/blob/v0.7.2/CHANGELOG.md)