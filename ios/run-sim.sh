#!/usr/bin/env bash
# Build the app for the simulator and launch it. Needs build-rust.sh first
# (SIM_ONLY=1 is enough) and xcodegen. DEVICE picks the simulator by name
# (newest iOS when the name repeats); BUZZ_VIM_NSEC logs in on launch.
set -euo pipefail
cd "$(dirname "$0")"
xcodegen generate --quiet
device=${DEVICE:-"iPhone 17 Pro"}
udid=$(xcrun simctl list devices available -j | python3 -c '
import json, sys
name = sys.argv[1]
found = [(rt, d["udid"]) for rt, ds in json.load(sys.stdin)["devices"].items()
         if "iOS" in rt for d in ds if d["name"] == name]
def ver(rt):
    return [int(x) for x in rt.rsplit("iOS-", 1)[-1].split("-") if x.isdigit()]
print(max(found, key=lambda f: ver(f[0]))[1] if found else "")
' "$device")
[ -n "$udid" ] || { echo "no simulator named $device"; exit 1; }
xcodebuild -project BuzzVim.xcodeproj -scheme BuzzVim -configuration Debug \
  -destination "id=$udid" -derivedDataPath build \
  CODE_SIGNING_ALLOWED=NO build | tail -n 30
app=build/Build/Products/Debug-iphonesimulator/BuzzVim.app
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl install "$udid" "$app"
SIMCTL_CHILD_BUZZ_VIM_NSEC="${BUZZ_VIM_NSEC:-}" xcrun simctl launch "$udid" com.lucianhymer.buzzvim
