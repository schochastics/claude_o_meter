#!/usr/bin/env bash
# Build, bundle, and ad-hoc sign claude_o_meter.app.
# Usage: ./scripts/build_app.sh [--open]
set -euo pipefail

cd "$(dirname "$0")/.."

cargo bundle --release
APP="target/release/bundle/osx/claude_o_meter.app"

codesign --force --deep --sign - "$APP"
echo "Built and signed: $APP"

if [[ "${1:-}" == "--open" ]]; then
  open "$APP"
fi
