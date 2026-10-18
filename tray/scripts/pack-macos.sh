#!/bin/sh
# Empacota "1fluxo Bridge.app" (sem instalador, sem notarização) e um zip para descarregar.
# Uso: tray/scripts/pack-macos.sh   (a partir de qualquer pasta)
# Sem conta Apple Developer: a app leva só uma assinatura ad-hoc, por isso na primeira
# abertura o Gatekeeper pede «Abrir mesmo assim» (Definições do Sistema → Privacidade).
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/dist"
APP="$OUT/1fluxo Bridge.app"
export PATH="$HOME/.cargo/bin:$PATH"

cargo build --release --manifest-path "$ROOT/Cargo.toml" -p fluxo-bridge-app
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/target/release/1fluxo-bridge" "$APP/Contents/MacOS/1fluxo-bridge"

# Ícone .icns a partir do ícone da app (iconutil e sips vêm com o macOS)
ICONSET="$(mktemp -d)/icon.iconset" && mkdir -p "$ICONSET"
SRC="$ROOT/icons/app-icon.png"
for s in 16 32 128 256; do
  sips -z $s $s "$SRC" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) "$SRC" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
cp "$SRC" "$ICONSET/icon_512x512.png"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/icon.icns"

VERSION="$(grep -m1 '^version' "$ROOT/app/Cargo.toml" | cut -d'"' -f2)"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>1fluxo Bridge</string>
  <key>CFBundleDisplayName</key><string>1fluxo Bridge</string>
  <key>CFBundleIdentifier</key><string>com.1fluxo.bridge</string>
  <key>CFBundleExecutable</key><string>1fluxo-bridge</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST

codesign --force --deep --sign - "$APP" >/dev/null 2>&1 && echo "assinatura ad-hoc aplicada"
(cd "$OUT" && rm -f 1fluxo-bridge-macos.zip && ditto -c -k --keepParent "1fluxo Bridge.app" 1fluxo-bridge-macos.zip)
echo "pronto: $OUT/1fluxo-bridge-macos.zip"
