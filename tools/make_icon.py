"""Generate assets/icon.ico with the standard library only. 仅用标准库生成 assets/icon.ico。"""
import struct, zlib, os

BG = (245, 158, 11)      # orange 橙色
FG = (255, 255, 255)     # white 白色
SS = 4                   # supersampling factor 超采样倍数


def inside_bg(x, y):
    r = 0.22
    cx = min(max(x, r), 1 - r)
    cy = min(max(y, r), 1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def inside_arrow(x, y):
    # Top arrow points right, bottom arrow points left. 上箭头向右，下箭头向左。
    for cy, direction in ((0.36, 1), (0.64, -1)):
        u = x if direction == 1 else 1 - x
        v = abs(y - cy)
        if 0.22 <= u <= 0.60 and v <= 0.055:
            return True
        if 0.56 <= u <= 0.80 and v <= (0.80 - u) * 0.62:
            return True
    return False


def render(size):
    px = []
    for j in range(size):
        row = []
        for i in range(size):
            a = f = 0
            for sj in range(SS):
                for si in range(SS):
                    x = (i + (si + 0.5) / SS) / size
                    y = (j + (sj + 0.5) / SS) / size
                    if inside_bg(x, y):
                        a += 1
                        f += inside_arrow(x, y)
            n = SS * SS
            if a == 0:
                row.append((0, 0, 0, 0))
            else:
                t = f / a
                rgb = [round(BG[k] * (1 - t) + FG[k] * t) for k in range(3)]
                row.append((rgb[0], rgb[1], rgb[2], round(255 * a / n)))
        px.append(row)
    return px


def png(px):
    size = len(px)
    raw = b''.join(b'\x00' + b''.join(bytes(p) for p in row) for row in px)

    def chunk(tag, data):
        return struct.pack('>I', len(data)) + tag + data + struct.pack('>I', zlib.crc32(tag + data))
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b''))


def bmp(px):
    size = len(px)
    head = struct.pack('<IiiHHIIiiII', 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    xor = b''.join(bytes((p[2], p[1], p[0], p[3])) for row in reversed(px) for p in row)
    mask = bytes(((size + 31) // 32) * 4) * size
    return head + xor + mask


images = [(s, png(render(s)) if s == 256 else bmp(render(s))) for s in (16, 20, 24, 32, 48, 256)]
out = struct.pack('<HHH', 0, 1, len(images))
offset = 6 + 16 * len(images)
for s, data in images:
    out += struct.pack('<BBBBHHII', s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
out += b''.join(d for _, d in images)
path = os.path.join(os.path.dirname(__file__), '..', 'assets', 'icon.ico')
with open(path, 'wb') as fh:
    fh.write(out)
print('wrote', os.path.normpath(path), len(out), 'bytes')
