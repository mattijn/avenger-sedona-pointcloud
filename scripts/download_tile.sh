#!/usr/bin/env sh
# Download the IGN LiDAR HD tile used in the examples (about 105 MB).
# The same file is used by the Giro3D "massive point cloud" example.
set -eu

TILE="LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz"
URL="https://3d.oslandia.com/giro3d/pointclouds/lidarhd/paris/${TILE}"
DEST="$(dirname "$0")/../data/${TILE}"

mkdir -p "$(dirname "$DEST")"
if [ -f "$DEST" ]; then
  echo "Already downloaded: $DEST"
else
  curl -fL --progress-bar -o "$DEST" "$URL"
  echo "Saved $DEST"
fi
