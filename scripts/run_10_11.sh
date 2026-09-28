#!/usr/bin/env sh
# Run experiments 10 (Mosaic flights) and 11 (CloudLasso) and open the images.
# Downloads the data first if it is missing. Images go to out/, so the
# committed ones in experiments/*/images/ are left alone.
#
#   ./scripts/run_10_11.sh          # both
#   ./scripts/run_10_11.sh 10       # only experiment 10
#   ./scripts/run_10_11.sh 11       # only experiment 11
set -eu

cd "$(dirname "$0")/.."
WHICH="${1:-all}"

TILE="data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz"
FLIGHTS="data/flights-10m.parquet"
FLIGHTS_URL="https://pub-1da360b43ceb401c809f68ca37c7f8a4.r2.dev/data/flights-10m.parquet"

# This machine runs Rust 1.89; see CLAUDE.md.
export CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback

# Build both together, so they share one set of compiled dependencies; building
# them one at a time recompiles DataFusion for each. Quick after the first time.
echo "Building (the first time takes a few minutes)..."
cargo build --release -q -p lidar-flights -p lidar-cloudlasso

if [ "$WHICH" = all ] || [ "$WHICH" = 10 ]; then
  if [ ! -f "$FLIGHTS" ]; then
    echo "Downloading the flights (about 72 MB)..."
    mkdir -p data
    curl -fL --progress-bar -o "$FLIGHTS" "$FLIGHTS_URL"
  fi
  echo "== Experiment 10: Mosaic flights"
  ./target/release/flights "$FLIGHTS" out/exp10
fi

if [ "$WHICH" = all ] || [ "$WHICH" = 11 ]; then
  [ -f "$TILE" ] || ./scripts/download_tile.sh
  echo "== Experiment 11: CloudLasso"
  ./target/release/cloudlasso "$TILE" out/exp11
fi

IMAGES=""
[ "$WHICH" = 11 ] || IMAGES="$IMAGES out/exp10/*.png"
[ "$WHICH" = 10 ] || IMAGES="$IMAGES out/exp11/*.png"
echo "Opening:" $IMAGES
# shellcheck disable=SC2086
open $IMAGES
