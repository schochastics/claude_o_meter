#!/usr/bin/env bash
# Build, bundle, and ad-hoc sign Claude-O-Meter.app.
# Usage: ./scripts/build_app.sh [--open]
set -euo pipefail

cd "$(dirname "$0")/.."

cargo bundle --release
APP="target/release/bundle/osx/Claude-O-Meter.app"

codesign --force --deep --sign - "$APP"
echo "Built and signed: $APP"

if [[ "${1:-}" == "--open" ]]; then
  open "$APP"
fi
