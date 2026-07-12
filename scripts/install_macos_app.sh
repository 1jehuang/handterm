#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BINARY=${1:-"$ROOT/target/release/handterm"}
APP_DIR=${2:-"$HOME/Applications/HandTerm.app"}
CONTENTS="$APP_DIR/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"

if [ ! -x "$BINARY" ]; then
  echo "HandTerm binary not found or not executable: $BINARY" >&2
  echo "Build it first with: cargo build --release" >&2
  exit 1
fi

mkdir -p "$MACOS" "$RESOURCES"
cp "$BINARY" "$RESOURCES/handterm"
chmod 755 "$RESOURCES/handterm"

cat > "$MACOS/HandTerm" <<'LAUNCHER'
#!/bin/sh
cd "$HOME"
exec "$(dirname "$0")/../Resources/handterm" --backend gpu "$@"
LAUNCHER
chmod 755 "$MACOS/HandTerm"

cat > "$CONTENTS/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key>
  <string>HandTerm</string>
  <key>CFBundleExecutable</key>
  <string>HandTerm</string>
  <key>CFBundleIdentifier</key>
  <string>com.jcode.handterm</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>HandTerm</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

plutil -lint "$CONTENTS/Info.plist"
/usr/bin/codesign --force --deep --sign - "$APP_DIR"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP_DIR"
mdimport "$APP_DIR" >/dev/null 2>&1 || true

echo "Installed HandTerm at $APP_DIR"
