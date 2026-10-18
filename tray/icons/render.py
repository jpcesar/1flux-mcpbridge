#!/usr/bin/env python3
"""Rasteriza os ícones do tray a partir do ícone da app pós-redesign («Repartição»,
1flux-app: src/app/icon.png, docs/redesenho/desenhos/Icone e loaders.dc.html), sem dependências.

Cada barra é uma pílula clara (o total) com uma pílula escura por cima (a parte), todas a
acabar na mesma linha de base. Geometria medida no ícone 512×512 da app; cores do design.

Uso: python3 render.py
Gera tray-{off,on,connected}.png (modelo monocromático, 128 px: preto + alfa, o macOS ajusta ao
modo claro/escuro) e color-{off,on,connected}.png (cores da marca, 128 px, para o Windows).
"""
import struct, zlib

# (x1, x2, topo da pílula clara, topo da pílula escura) — base comum em y=399, largura 66
BARS = [(62, 128, 230, 267), (164, 230, 282, 324), (266, 332, 143, 215), (369, 435, 199, 283)]
BASE = 399
# (cor escura, cor clara) por barra: teal, coral, roxo, taupe
PALETTE = [("#2B6E73", "#CFE4E3"), ("#E5714F", "#F9D3C6"), ("#7A5FB8", "#E3DAF3"), ("#A08F78", "#EBE3D6")]
# Caixa quadrada em torno das barras, em unidades do ícone de 512
VB_X, VB_Y, VB = 56, 79, 384
DOT = (402, 123, 36)  # centro x, y e raio do ponto de «ligado», acima da última barra
SS = 4  # supersampling por eixo


def hex_rgb(h):
    return tuple(int(h[i : i + 2], 16) for i in (1, 3, 5))


def in_pill(px, py, x1, x2, top):
    r = (x2 - x1) / 2
    if px < x1 or px > x2 or py < top or py > BASE:
        return False
    cx = (x1 + x2) / 2
    if py < top + r:
        return (px - cx) ** 2 + (py - (top + r)) ** 2 <= r * r
    if py > BASE - r:
        return (px - cx) ** 2 + (py - (BASE - r)) ** 2 <= r * r
    return True


def sample(ux, uy, dark_of, light_of, dot):
    """Devolve (rgb, alfa) do ponto, ou None. A pílula escura tapa a clara."""
    if dot and (ux - DOT[0]) ** 2 + (uy - DOT[1]) ** 2 <= DOT[2] ** 2:
        return dot
    for i, (x1, x2, top_l, top_d) in enumerate(BARS):
        if in_pill(ux, uy, x1, x2, top_d):
            return dark_of(i)
        if in_pill(ux, uy, x1, x2, top_l):
            return light_of(i)
    return None


def render(size, dark_of, light_of, dot=None):
    rows = []
    for y in range(size):
        row = bytearray()
        for x in range(size):
            ar = ag = ab = aa = 0.0
            for sy in range(SS):
                for sx in range(SS):
                    ux = VB_X + (x + (sx + 0.5) / SS) * VB / size
                    uy = VB_Y + (y + (sy + 0.5) / SS) * VB / size
                    hit = sample(ux, uy, dark_of, light_of, dot)
                    if hit:
                        (r, g, b), a = hit
                        ar += r * a
                        ag += g * a
                        ab += b * a
                        aa += a
            a = aa / (SS * SS)
            r, g, b = (round(v / aa) for v in (ar, ag, ab)) if aa else (0, 0, 0)
            row += bytes((r, g, b, round(a * 255)))
        rows.append(b"\x00" + bytes(row))
    return rows


def write_png(path, size, rows):
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")
    open(path, "wb").write(png)


BLACK = (0, 0, 0)
TEAL = hex_rgb("#2B6E73")

# Monocromático: a parte em preto cheio, o total mais leve (o sistema pinta o resto).
mono = {
    "off": (0.30, 0.10, None),
    "on": (1.00, 0.30, None),
    "connected": (1.00, 0.30, (BLACK, 1.0)),
}
for name, (a_dark, a_light, dot) in mono.items():
    write_png(f"tray-{name}.png", 128, render(128, lambda i, a=a_dark: (BLACK, a), lambda i, a=a_light: (BLACK, a), dot))

# Cores da marca; «off» esbatido, «connected» com um ponto teal.
color = {"off": (0.40, None), "on": (1.0, None), "connected": (1.0, (TEAL, 1.0))}
for name, (k, dot) in color.items():
    write_png(
        f"color-{name}.png",
        128,
        render(128, lambda i, k=k: (hex_rgb(PALETTE[i][0]), k), lambda i, k=k: (hex_rgb(PALETTE[i][1]), k), dot),
    )
print("ok")
