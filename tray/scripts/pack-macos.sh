#!/bin/sh
# Empacota "1fluxo Bridge.app" (sem instalador) e um zip para descarregar.
# Uso: tray/scripts/pack-macos.sh   (a partir de qualquer pasta)
#
# Assinatura:
# · Com MACOS_SIGN_IDENTITY ("Developer ID Application: Nome (TEAMID)", no keychain):
#   assina com hardened runtime e timestamp.
# · Com NOTARY_KEY_PATH, NOTARY_KEY_ID e NOTARY_ISSUER_ID (chave da App Store Connect
#   API): notariza na Apple, agrafa o ticket à app e volta a fazer o zip.
# · Sem nada disto (build local): assinatura ad-hoc, e na primeira abertura o
#   Gatekeeper pede «Abrir mesmo assim» (Definições do Sistema → Privacidade).
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

ZIP="$OUT/1fluxo-bridge-macos.zip"
make_zip() {
  (cd "$OUT" && rm -f "$ZIP" && ditto -c -k --keepParent "1fluxo Bridge.app" "$ZIP")
}

if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
  # Um só executável no bundle: assinar o bundle assina-o (sem --deep, que a
  # Apple desaconselha). Hardened runtime e timestamp são obrigatórios para notarizar.
  codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$APP"
  codesign --verify --strict --verbose=2 "$APP"
  echo "assinada com Developer ID"
else
  codesign --force --sign - "$APP" >/dev/null 2>&1 && echo "assinatura ad-hoc aplicada"
fi
make_zip

if [ -n "${NOTARY_KEY_PATH:-}" ]; then
  : "${NOTARY_KEY_ID:?falta NOTARY_KEY_ID}" "${NOTARY_ISSUER_ID:?falta NOTARY_ISSUER_ID}"
  [ -n "${MACOS_SIGN_IDENTITY:-}" ] || { echo "notarizar exige MACOS_SIGN_IDENTITY"; exit 1; }
  # O notarytool sai com 0 mesmo quando a Apple recusa: confirma-se o estado.
  RESULT="$(xcrun notarytool submit "$ZIP" --key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" \
    --issuer "$NOTARY_ISSUER_ID" --wait --output-format json)"
  echo "$RESULT"
  case "$RESULT" in
    *'"status":"Accepted"'*) ;;
    *)
      ID="$(printf '%s' "$RESULT" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')"
      [ -n "$ID" ] && xcrun notarytool log "$ID" --key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" \
        --issuer "$NOTARY_ISSUER_ID" || true
      echo "a notarização falhou"; exit 1
      ;;
  esac
  # O ticket fica dentro da app: abre sem rede na primeira vez.
  xcrun stapler staple "$APP"
  spctl --assess --type execute --verbose=2 "$APP"
  make_zip
  echo "notarizada e agrafada"
fi

echo "pronto: $ZIP"
