#!/bin/sh
# Run ZTile straight from this folder, without installing (builds it the first time).
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
if [ ! -x "$HERE/target/release/ztile" ]; then
    (cd "$HERE" && cargo build --release --locked)
fi
exec "$HERE/target/release/ztile" "$@"
