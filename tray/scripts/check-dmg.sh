#!/bin/sh
# Confirma que o DMG gerado pelo pack-macos.sh monta e tem o que a janela precisa:
# a app (com assinatura válida), o atalho para Aplicações, o fundo e o .DS_Store do layout.
# Uso: tray/scripts/check-dmg.sh [caminho do .dmg]   (por defeito, tray/dist/1fluxo-bridge-macos.dmg)
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DMG="${1:-$ROOT/dist/1fluxo-bridge-macos.dmg}"

hdiutil verify -quiet "$DMG"
MNT="$(mktemp -d)"
trap 'hdiutil detach "$MNT" -quiet -force 2>/dev/null || true; rmdir "$MNT" 2>/dev/null || true' EXIT
hdiutil attach -nobrowse -readonly -quiet -mountpoint "$MNT" "$DMG"

[ -d "$MNT/1fluxo Bridge.app" ] || { echo "falta a app no DMG"; exit 1; }
[ "$(readlink "$MNT/Applications")" = "/Applications" ] || { echo "falta o atalho para Aplicações"; exit 1; }
[ -s "$MNT/.background/background.tiff" ] || { echo "falta o fundo"; exit 1; }
[ -s "$MNT/.DS_Store" ] || { echo "falta o .DS_Store do layout"; exit 1; }
codesign --verify --strict "$MNT/1fluxo Bridge.app"
echo "DMG ok: $DMG"
