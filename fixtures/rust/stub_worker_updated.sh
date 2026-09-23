#!/bin/sh
# Same as stub_worker.sh, but the `research` subcommand emits the "updated"
# fixture (fixtures/rust/semiconductor_v2_updated.json): the same dossier
# with one changed source content_hash and one additional claim. Used by
# the HTTP-level as-of proof in tests/roundtrip.rs.
set -eu

subcommand="$1"
dir="$(cd "$(dirname "$0")" && pwd)"

cat >/dev/null

case "$subcommand" in
  research)
    cat "$dir/semiconductor_v2_updated.json"
    ;;
  classify-family)
    cat "$dir/classify_family_response.json"
    ;;
  *)
    echo "stub_worker_updated.sh: unknown subcommand: $subcommand" >&2
    exit 1
    ;;
esac
