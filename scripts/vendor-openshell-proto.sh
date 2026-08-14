#!/usr/bin/env bash
# Vendor OpenShell .proto files from a pinned upstream tag.
#
# Usage: scripts/vendor-openshell-proto.sh <tag>
# Example: scripts/vendor-openshell-proto.sh v0.0.50
#
# Re-pulls datamodel.proto, sandbox.proto, openshell.proto and their
# upstream-local imports (options.proto since v0.0.104) from
# into crates/right-openshell/proto/openshell/ and writes the tag +
# fetch timestamp into crates/right-openshell/proto/UPSTREAM.md.
set -euo pipefail

TAG="${1:?usage: $0 <tag>  (e.g. v0.0.50)}"
DEST_DIR="crates/right-openshell/proto/openshell"
UPSTREAM_FILE="crates/right-openshell/proto/UPSTREAM.md"

if [[ ! -d "$DEST_DIR" ]]; then
    echo "error: $DEST_DIR not found; run from repo root" >&2
    exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

for f in datamodel.proto sandbox.proto openshell.proto options.proto; do
    url="https://raw.githubusercontent.com/NVIDIA/OpenShell/${TAG}/proto/${f}"
    echo "fetching $url"
    curl -fsSL "$url" -o "$TMP/$f"
done

rm -f "$DEST_DIR"/*.proto
mv "$TMP"/*.proto "$DEST_DIR/"

printf 'tag: %s\nfetched: %s\nupstream: https://github.com/NVIDIA/OpenShell\n' \
    "$TAG" "$(date -u +%FT%TZ)" > "$UPSTREAM_FILE"

echo "Vendored OpenShell proto from $TAG"
echo "Run: cargo check -p right-openshell  to regenerate tonic stubs"
