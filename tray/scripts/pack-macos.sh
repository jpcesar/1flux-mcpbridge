#!/bin/sh
# Empacota "1fluxo Bridge.app" e gera dois ficheiros para descarregar: o
# 1fluxo-bridge-macos.dmg (arrastar para Aplicações) e o 1fluxo-bridge-macos.zip.
# Uso: tray/scripts/pack-macos.sh   (a partir de qualquer pasta)
#
# Assinatura:
# · Com MACOS_SIGN_IDENTITY ("Developer ID Application: Nome (TEAMID)", no keychain):
#   assina a app e o DMG com hardened runtime e timestamp.
# · Com NOTARY_KEY_PATH, NOTARY_KEY_ID e NOTARY_ISSUER_ID (chave da App Store Connect
#   API): notariza na Apple a app (via zip) e depois o DMG, e agrafa o ticket a cada um.
# · Sem nada disto (build local): assinatura ad-hoc na app e DMG sem assinar; na
#   primeira abertura o Gatekeeper pede «Abrir mesmo assim» (Definições do Sistema →
#   Privacidade).
#
# O aspecto da janela do DMG vem de icons/dmg.DS_Store (gerado por scripts/dmg-layout.sh).
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
  <key>LSMinimumSystemVersion</key><string>13.0</string>
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

# Envia FICHEIRO à Apple e espera. O notarytool sai com 0 mesmo quando a Apple recusa:
# confirma-se o estado.
notarize() {
  RESULT="$(xcrun notarytool submit "$1" --key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" \
    --issuer "$NOTARY_ISSUER_ID" --wait --output-format json)"
  echo "$RESULT"
  case "$RESULT" in
    *'"status":"Accepted"'*) ;;
    *)
      ID="$(printf '%s' "$RESULT" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')"
      [ -n "$ID" ] && xcrun notarytool log "$ID" --key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" \
        --issuer "$NOTARY_ISSUER_ID" || true
      echo "a notarização de $(basename "$1") falhou"; exit 1
      ;;
  esac
}

NOTARIZE=0
if [ -n "${NOTARY_KEY_PATH:-}" ]; then
  : "${NOTARY_KEY_ID:?falta NOTARY_KEY_ID}" "${NOTARY_ISSUER_ID:?falta NOTARY_ISSUER_ID}"
  [ -n "${MACOS_SIGN_IDENTITY:-}" ] || { echo "notarizar exige MACOS_SIGN_IDENTITY"; exit 1; }
  NOTARIZE=1
  notarize "$ZIP"
  # O ticket fica dentro da app: abre sem rede na primeira vez. O DMG e o zip levam-na já agrafada.
  xcrun stapler staple "$APP"
  spctl --assess --type execute --verbose=2 "$APP"
  make_zip
  echo "app notarizada e agrafada"
fi

# DMG: a app, o atalho para Aplicações e o fundo, com a janela do modelo. O nome do
# volume e o caminho do fundo têm de ser os de scripts/dmg-layout.sh (o .DS_Store
# guarda-os). Sem hdiutil/AppleScript no CI: tudo vem da pasta de staging.
DMG="$OUT/1fluxo-bridge-macos.dmg"
VOLNAME="1fluxo Bridge"
STAGE="$(mktemp -d)/dmg"
mkdir -p "$STAGE/.background"
ditto "$APP" "$STAGE/1fluxo Bridge.app"
ln -s /Applications "$STAGE/Applications"
tiffutil -cathidpicheck "$ROOT/icons/dmg-background.png" "$ROOT/icons/dmg-background@2x.png" \
  -out "$STAGE/.background/background.tiff" >/dev/null 2>&1
cp "$ROOT/icons/dmg.DS_Store" "$STAGE/.DS_Store"
rm -f "$DMG"
# O hdiutil falha de vez em quando com «Resource busy» nos runners do CI: tenta-se de novo.
n=0
until hdiutil create -quiet -srcfolder "$STAGE" -volname "$VOLNAME" -fs HFS+ -format UDZO \
  -imagekey zlib-level=9 "$DMG"; do
  n=$((n + 1)); [ $n -lt 4 ] || { echo "o hdiutil falhou"; exit 1; }
  sleep 5
done
rm -rf "$(dirname "$STAGE")"

if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
  codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$DMG"
  codesign --verify --verbose=2 "$DMG"
  echo "DMG assinado com Developer ID"
fi
if [ "$NOTARIZE" = 1 ]; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
  echo "DMG notarizado e agrafado"
fi

echo "pronto: $DMG e $ZIP"
