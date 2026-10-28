#!/bin/bash
# Builds the Rust app core (rust/core) for the Apple targets and writes:
#   ios/RustCore/SwarmPanelCore.xcframework   static libs + C header/module map
#   ios/RustCore/SwarmPanelCore.swift         UniFFI-generated Swift API
# Needs macOS with Xcode. PLATFORMS="ios tvos" (default) or a subset.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
RUST="$ROOT/rust"
OUT="$ROOT/ios/RustCore"
PLATFORMS="${PLATFORMS:-ios tvos}"
export IPHONEOS_DEPLOYMENT_TARGET=16.0
export TVOS_DEPLOYMENT_TARGET=17.0

cd "$RUST"
LIBS=()
staticlib() { # toolchain target
  echo "==> $2"
  if [ "$1" = "nightly" ]; then
    cargo +nightly rustc -Z build-std=std,panic_abort --release -p swarmpanel-core --lib --crate-type staticlib --target "$2"
  else
    cargo rustc --release -p swarmpanel-core --lib --crate-type staticlib --target "$2"
  fi
}

if [[ " $PLATFORMS " == *" ios "* ]]; then
  rustup target add aarch64-apple-ios aarch64-apple-ios-sim
  staticlib stable aarch64-apple-ios
  staticlib stable aarch64-apple-ios-sim
  LIBS+=(aarch64-apple-ios aarch64-apple-ios-sim)
fi
if [[ " $PLATFORMS " == *" tvos "* ]]; then
  # tvOS is a tier-3 Rust target: std is built from source on nightly.
  rustup toolchain install nightly --profile minimal --component rust-src
  staticlib nightly aarch64-apple-tvos
  staticlib nightly aarch64-apple-tvos-sim
  LIBS+=(aarch64-apple-tvos aarch64-apple-tvos-sim)
fi

echo "==> Swift bindings"
cargo build --release -p swarmpanel-core --lib
GEN="$RUST/target/uniffi-gen"
rm -rf "$GEN" && mkdir -p "$GEN/include"
cargo run --release -p swarmpanel-core --features bindgen --bin uniffi-bindgen -- \
  generate --library "$RUST/target/release/libswarmpanel_core.dylib" --language swift \
  --config "$RUST/core/uniffi.toml" --out-dir "$GEN"
cp "$GEN/SwarmPanelCoreFFI.h" "$GEN/include/"
cp "$GEN/SwarmPanelCoreFFI.modulemap" "$GEN/include/module.modulemap"

echo "==> XCFramework"
rm -rf "$OUT" && mkdir -p "$OUT"
ARGS=()
for t in "${LIBS[@]}"; do ARGS+=(-library "$RUST/target/$t/release/libswarmpanel_core.a" -headers "$GEN/include"); done
xcodebuild -create-xcframework "${ARGS[@]}" -output "$OUT/SwarmPanelCore.xcframework"
cp "$GEN/SwarmPanelCore.swift" "$OUT/"
du -sh "$OUT"/*
