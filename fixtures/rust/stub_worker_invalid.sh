#!/bin/sh
# Same as stub_worker.sh, but the `research` subcommand emits a
# deliberately invalid payload (a dangling evidence.claim_id reference), to
# prove POST /api/research surfaces a worker-produced validation failure as
# a 502 (docs/contracts.md §C: "502 worker failure").
set -eu

subcommand="$1"
dir="$(cd "$(dirname "$0")" && pwd)"

cat >/dev/null

case "$subcommand" in
  research)
    cat "$dir/semiconductor_v2_invalid.json"
    ;;
  *)
    echo "stub_worker_invalid.sh: unknown subcommand: $subcommand" >&2
    exit 1
    ;;
esac
