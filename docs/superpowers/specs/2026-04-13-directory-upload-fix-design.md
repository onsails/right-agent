# Directory Upload Fix Design

## Problem

`upload_file()` in `openshell.rs` passes directories to `openshell sandbox upload` CLI,
which silently drops files (known OpenShell bug). This breaks sync of builtin skills
(rightcron, rightskills, rightmcp) — none of them land in the sandbox.

## Solution

Make `upload_file()` detect directory sources and decompose them into parallel
single-file uploads. Single-file uploads are reliable (verified by existing tests).

## Changes

### `crates/rightclaw/src/openshell.rs` — `upload_file()`

Current behavior: passes source path directly to `openshell sandbox upload`.

New behavior:
1. If source is a file → unchanged (one CLI call)
2. If source is a directory → `walkdir` to collect all files, compute relative paths,
   upload each file individually via `openshell sandbox upload` in parallel

Parallel upload:
```rust
futures::stream::iter(uploads)
    .buffer_unordered(10)
    .collect::<Vec<_>>()
```

Error handling: collect all results, return error if any upload failed. Use `?` on
each result so the first error propagates.

### No changes to callers

`sync.rs`, `openshell.rs` staging — all call `upload_file()` with directories already.
The fix is transparent.

### No verify step

Single-file uploads are reliable (3 passing integration tests). No post-upload
verification needed.

## Testing

Existing red test `upload_directory_preserves_files` must turn green.

## Dependencies

- `walkdir` — already in workspace deps
- `futures` — already in workspace deps (for `buffer_unordered`)
