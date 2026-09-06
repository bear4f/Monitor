#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
server_bin="${MONITOR_SERVER_BIN:-$repo_dir/target/release/monitor-server}"
bench_tmp="$(mktemp -d "${TMPDIR:-/tmp}/monitor-bench-status.XXXXXX")"
trap 'rm -rf -- "$bench_tmp"' EXIT INT TERM

command -v python3 >/dev/null 2>&1 || {
  echo "python3 is required (standard library only)" >&2
  exit 1
}

if [[ ! -x "$server_bin" ]]; then
  command -v cargo >/dev/null 2>&1 || {
    echo "cargo is required to build monitor-server" >&2
    exit 1
  }
  cargo build --release -p monitor-server
fi

python3 "$repo_dir/scripts/bench.py" status \
  --server "$server_bin" \
  --work-dir "$bench_tmp" \
  --seconds "${BENCH_STATUS_SECONDS:-30}" \
  --concurrency "${BENCH_STATUS_CONCURRENCY:-32}"
