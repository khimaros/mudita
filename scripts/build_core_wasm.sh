#!/usr/bin/env bash
# build the mudita core for a web page (the wasm/ crate) into web/wasm/ as
# mudita.js + mudita_bg.wasm, beside a snippets/ directory holding the
# javascript the crate brings along: all three are served together.
# wasm-bindgen must be the release the crate is built against, or the
# browser glue will not line up.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/web/wasm"
OUT_NAME="mudita"
TARGET="wasm32-unknown-unknown"
# a debug build of a veilid node is several times the size and far slower.
PROFILE="release"
BUILT="$ROOT/target/$TARGET/$PROFILE/mudita_wasm.wasm"

# keep this in step with the wasm-bindgen version in wasm/Cargo.toml.
WBGEN_VERSION="0.2.121"

# rust tools come from mise. under mise, cargo's home is the per-version rust
# dir, so `cargo install` tools (wasm-bindgen-cli) land in its bin. set it
# explicitly and put it on PATH so a rust version bump does not orphan them.
export CARGO_HOME="$(mise where rust 2>/dev/null)"
export PATH="$CARGO_HOME/bin:$HOME/.cargo/bin:$PATH"

rustup target add "$TARGET"
if ! wasm-bindgen --version 2>/dev/null | grep -q "$WBGEN_VERSION"; then
  echo "installing wasm-bindgen-cli $WBGEN_VERSION ..."
  cargo install wasm-bindgen-cli --version "$WBGEN_VERSION"
fi

echo "compiling the mudita core to wasm ..."
( cd "$ROOT" && cargo build --locked --"$PROFILE" --target "$TARGET" -p mudita-wasm )

mkdir -p "$OUT"
wasm-bindgen --out-dir "$OUT" --out-name "$OUT_NAME" --target web "$BUILT"
echo "wrote the mudita core wasm to $OUT"
