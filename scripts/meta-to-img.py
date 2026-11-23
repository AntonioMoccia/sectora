#!/usr/bin/env python3
"""Ricostruisce un'immagine disco "vuota" da un file creato con `sectora meta`.

L'immagine ha la stessa dimensione del disco originale ed è sparse: contiene
solo le strutture salvate (avvio, FAT, directory, record MFT), tutto il resto
è a zero. Serve per riprodurre una scansione senza avere i dati dell'utente.

Uso: python3 scripts/meta-to-img.py strutture.meta disco.img
"""
import struct
import sys


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    src, dst = sys.argv[1], sys.argv[2]
    with open(src, "rb") as f, open(dst, "xb") as out:
        if f.read(9) != b"SECTMETA1":
            sys.exit("non è un file creato con `sectora meta`")
        (size,) = struct.unpack("<Q", f.read(8))
        out.truncate(size)
        records = total = 0
        while head := f.read(12):
            offset, length = struct.unpack("<QI", head)
            data = f.read(length)
            if len(data) != length:
                sys.exit("file troncato")
            out.seek(offset)
            out.write(data)
            records += 1
            total += length
    print(f"{dst}: {size / 2**30:.1f} GiB, {records} blocchi, {total / 2**20:.1f} MiB di strutture")


if __name__ == "__main__":
    main()
