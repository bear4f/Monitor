#!/usr/bin/env bash
set -euo pipefail

set +x
project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$project_root/web"
npm ci
npm run build

cd "$project_root"
cargo build --release
