#!/usr/bin/env sh
# Open the interactive windows of experiments 10 (Mosaic flights) and 11
# (CloudLasso). Downloads the data first if it is missing.
#
#   ./scripts/live_10_11.sh 10      # the flights window
#   ./scripts/live_10_11.sh 11      # the CloudLasso window
set -eu

cd "$(dirname "$0")/.."
WHICH="${1:-}"

TILE="data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz"
FLIGHTS="data/flights-10m.parquet"
FLIGHTS_URL="https://pub-1da360b43ceb401c809f68ca37c7f8a4.r2.dev/data/flights-10m.parquet"

# This machine runs Rust 1.89; see CLAUDE.md.
export CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback

case "$WHICH" in
  10)
    if [ ! -f "$FLIGHTS" ]; then
      echo "Downloading the flights (about 72 MB)..."
      mkdir -p data
      curl -fL --progress-bar -o "$FLIGHTS" "$FLIGHTS_URL"
    fi
    # Built together with experiment 11, as run_10_11.sh does, so neither recompiles the other's dependencies.
    cargo build --release -q -p lidar-flights -p lidar-cloudlasso
    exec ./target/release/flights_live "$FLIGHTS"
    ;;
  11)
    [ -f "$TILE" ] || ./scripts/download_tile.sh
    cargo build --release -q -p lidar-flights -p lidar-cloudlasso
    exec ./target/release/cloudlasso_live "$TILE"
    ;;
  *)
    echo "usage: $0 10|11" >&2
    exit 2
    ;;
esac
