#!/usr/bin/env bash
# Build the app for the simulator and launch it. Needs build-rust.sh first
# (SIM_ONLY=1 is enough) and xcodegen. BUZZ_VIM_NSEC logs in on launch.
set -euo pipefail
cd "$(dirname "$0")"
xcodegen generate --quiet
device=${DEVICE:-"iPhone 17 Pro"}
xcodebuild -project BuzzVim.xcodeproj -scheme BuzzVim -configuration Debug \
  -destination "platform=iOS Simulator,name=$device" -derivedDataPath build \
  CODE_SIGNING_ALLOWED=NO build | tail -n 30
app=build/Build/Products/Debug-iphonesimulator/BuzzVim.app
xcrun simctl boot "$device" 2>/dev/null || true
xcrun simctl install "$device" "$app"
SIMCTL_CHILD_BUZZ_VIM_NSEC="${BUZZ_VIM_NSEC:-}" xcrun simctl launch "$device" com.lucianhymer.buzzvim
