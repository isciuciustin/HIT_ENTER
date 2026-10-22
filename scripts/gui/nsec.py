"""Prints a fresh random nsec (bech32) — a throwaway test identity."""
import os

CH = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"


def polymod(v):
    g = [0x3B6A57B2, 0x26508E6D, 0x1EA119FA, 0x3D4233DD, 0x2A1462B3]
    c = 1
    for x in v:
        b = c >> 25
        c = (c & 0x1FFFFFF) << 5 ^ x
        for i in range(5):
            c ^= g[i] if (b >> i) & 1 else 0
    return c


def encode(hrp, data):
    acc = bits = 0
    out = []
    for b in data:
        acc = (acc << 8) | b
        bits += 8
        while bits >= 5:
            bits -= 5
            out.append((acc >> bits) & 31)
    if bits:
        out.append((acc << (5 - bits)) & 31)
    v = [ord(x) >> 5 for x in hrp] + [0] + [ord(x) & 31 for x in hrp] + out + [0] * 6
    m = polymod(v) ^ 1
    return hrp + "1" + "".join(CH[d] for d in out + [(m >> 5 * (5 - i)) & 31 for i in range(6)])


print(encode("nsec", os.urandom(32)))
