#!/bin/sh
# Gera tray/icons/dmg.DS_Store, o «modelo» com a janela do DMG: app à esquerda, atalho
# de Aplicações à direita e o fundo com a seta. Corre-se uma vez, num macOS com sessão
# gráfica (o Finder tem de aceitar AppleScript), sempre que mudar o fundo ou as posições.
# O pack-macos.sh só copia o resultado, por isso o CI não depende do Finder.
#
# O .DS_Store guarda o nome do volume e o caminho do fundo: ficam fixos em VOLNAME e
# .background/background.tiff, tal como no pack-macos.sh.
# Uso: tray/scripts/dmg-layout.sh
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VOLNAME="1fluxo Bridge"
APPNAME="1fluxo Bridge.app"
# Posições dos ícones e tamanho da janela (o fundo tem 660×400; a barra de título soma-se).
APP_X=165 APP_Y=190 LINK_X=495 LINK_Y=190
WIN_L=200 WIN_T=120 WIN_W=660 WIN_H=400

[ -d "/Volumes/$VOLNAME" ] && { echo "feche primeiro o volume «$VOLNAME» (está montado)"; exit 1; }

WORK="$(mktemp -d)"
trap 'hdiutil detach "/Volumes/$VOLNAME" -quiet -force 2>/dev/null || true; rm -rf "$WORK"' EXIT
STAGE="$WORK/stage"
mkdir -p "$STAGE/.background" "$STAGE/$APPNAME"
tiffutil -cathidpicheck "$ROOT/icons/dmg-background.png" "$ROOT/icons/dmg-background@2x.png" \
  -out "$STAGE/.background/background.tiff" >/dev/null
ln -s /Applications "$STAGE/Applications"

hdiutil create -quiet -srcfolder "$STAGE" -volname "$VOLNAME" -fs HFS+ -format UDRW "$WORK/layout.dmg"
hdiutil attach -quiet -noautoopen -readwrite "$WORK/layout.dmg"

osascript <<APPLESCRIPT
tell application "Finder"
  tell disk "$VOLNAME"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {$WIN_L, $WIN_T, $((WIN_L + WIN_W)), $((WIN_T + WIN_H + 28))}
    set opts to the icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to 128
    set text size of opts to 13
    set background picture of opts to file ".background:background.tiff"
    set position of item "$APPNAME" of container window to {$APP_X, $APP_Y}
    set position of item "Applications" of container window to {$LINK_X, $LINK_Y}
    update without registering applications
    delay 2
    close
  end tell
end tell
APPLESCRIPT

# O Finder grava o .DS_Store depois de fechar a janela, e por fases: espera-se que
# apareça o registo da janela (bwsp) e que o tamanho assente.
DS="/Volumes/$VOLNAME/.DS_Store"
i=0 last=-1 stable=0
while [ $i -lt 40 ]; do
  size=$(stat -f %z "$DS" 2>/dev/null || echo 0)
  if [ "$size" -gt 0 ] && [ "$size" = "$last" ] && grep -q bwsp "$DS"; then stable=$((stable + 1)); else stable=0; fi
  [ $stable -ge 3 ] && break
  last=$size; i=$((i + 1)); sleep 1
done
[ $stable -ge 3 ] || { echo "o Finder não gravou o .DS_Store completo"; exit 1; }
sync
cp "/Volumes/$VOLNAME/.DS_Store" "$ROOT/icons/dmg.DS_Store"
echo "gravado: $ROOT/icons/dmg.DS_Store"
