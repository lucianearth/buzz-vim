#!/usr/bin/env bash
# Build buzz-vim-core for iOS: BuzzVimCore.xcframework (device + simulator)
# and the Swift bindings in Generated/. Run on a Mac with the two rustup
# targets (aarch64-apple-ios, aarch64-apple-ios-sim). SIM_ONLY=1 skips the
# device slice for a quicker simulator loop.
set -euo pipefail
cd "$(dirname "$0")/.."
export IPHONEOS_DEPLOYMENT_TARGET=17.0
target_dir=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import sys,json;print(json.load(sys.stdin)["target_directory"])')
targets=(aarch64-apple-ios-sim)
[ "${SIM_ONLY:-}" = 1 ] || targets+=(aarch64-apple-ios)
for t in "${targets[@]}"; do
  cargo build --locked -p buzz-vim-ffi --lib --release --target "$t"
done

gen=ios/Generated
rm -rf "$gen" ios/BuzzVimCore.xcframework
mkdir -p "$gen/include"
cargo run --locked -q -p buzz-vim-ffi --bin uniffi-bindgen -- generate \
  --library "$target_dir/${targets[0]}/release/libbuzz_vim_ffi.a" \
  --language swift --out-dir "$gen"
mv "$gen/buzz_vim_ffiFFI.h" "$gen/include/"
mv "$gen/buzz_vim_ffiFFI.modulemap" "$gen/include/module.modulemap"

args=()
for t in "${targets[@]}"; do
  args+=(-library "$target_dir/$t/release/libbuzz_vim_ffi.a" -headers "$gen/include")
done
xcodebuild -create-xcframework "${args[@]}" -output ios/BuzzVimCore.xcframework
echo "built ios/BuzzVimCore.xcframework (${targets[*]}) and $gen/buzz_vim_ffi.swift"
