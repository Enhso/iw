#!/bin/sh
# Stub research worker for tests: ignores stdin and any trailing arguments
# (e.g. --fixture-dir DIR) and emits a canned v2 response keyed on the
# subcommand argument. Lets Rust-side tests exercise the stdin/stdout
# subcommand protocol (docs/contracts.md §A) without a Python environment.
set -eu

subcommand="$1"
dir="$(cd "$(dirname "$0")" && pwd)"

# Drain and discard stdin so the caller's write never blocks on a full pipe.
cat >/dev/null

case "$subcommand" in
  research)
    cat "$dir/semiconductor_v2.json"
    ;;
  classify-family)
    cat "$dir/classify_family_response.json"
    ;;
  *)
    echo "stub_worker.sh: unknown subcommand: $subcommand" >&2
    exit 1
    ;;
esac
