#!/usr/bin/env python3
"""Rasteriza o fundo da janela do DMG: uma seta da app (à esquerda) para o atalho de
Aplicações (à direita), sem dependências.

Uso: python3 render-dmg.py
Gera dmg-background.png (660×400) e dmg-background@2x.png (1320×800). O pack-macos.sh
junta os dois num TIFF com `tiffutil -cathidpicheck`, para o Finder escolher a versão
certa em ecrãs retina. As posições dos ícones (165,190 e 495,190) estão em
tray/scripts/dmg-layout.sh, que gera o .DS_Store de modelo.
"""
import struct, zlib

W, H = 660, 400
SS = 4  # supersampling por eixo
BG = (0xF7, 0xF4, 0xEE)  # creme claro, o mesmo tom do papel do 1fluxo
INK = (0x2B, 0x6E, 0x73)  # teal da marca

# Seta entre os dois ícones (centrados a y=190, com 128 px): haste e ponta, em unidades 1x.
CY = 190
SHAFT_X1, SHAFT_X2, SHAFT_HALF = 262, 380, 5
HEAD_X1, HEAD_X2, HEAD_HALF = 366, 412, 26


def in_arrow(x, y):
    # Haste com as pontas arredondadas.
    if SHAFT_X1 <= x <= SHAFT_X2 and abs(y - CY) <= SHAFT_HALF:
        return True
    for cx in (SHAFT_X1, SHAFT_X2):
        if (x - cx) ** 2 + (y - CY) ** 2 <= SHAFT_HALF**2:
            return True
    # Ponta triangular (vértice à direita).
    if HEAD_X1 <= x <= HEAD_X2:
        reach = HEAD_HALF * (HEAD_X2 - x) / (HEAD_X2 - HEAD_X1)
        return abs(y - CY) <= reach
    return False


def render(scale):
    rows = []
    for py in range(H * scale):
        row = bytearray()
        for px in range(W * scale):
            hits = 0
            for sy in range(SS):
                for sx in range(SS):
                    x = (px + (sx + 0.5) / SS) / scale
                    y = (py + (sy + 0.5) / SS) / scale
                    hits += in_arrow(x, y)
            k = 0.85 * hits / (SS * SS)
            row += bytes(round(b * (1 - k) + i * k) for b, i in zip(BG, INK))
        rows.append(b"\x00" + bytes(row))
    return rows


def write_png(path, width, height, rows):
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)  # RGB, 8 bits
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")
    open(path, "wb").write(png)


write_png("dmg-background.png", W, H, render(1))
write_png("dmg-background@2x.png", W * 2, H * 2, render(2))
print("ok")
