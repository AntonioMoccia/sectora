#!/usr/bin/env bash
# Prova realistica: crea una "chiavetta" FAT32 in un file immagine, ci copia
# delle foto, le cancella, riformatta il volume e poi verifica che
# `sectora carve` le ritrovi con hash identico.
#
# Requisiti: dosfstools + mtools, python3 con Pillow.
#   Linux:  sudo apt install dosfstools mtools && pip install pillow
#   macOS:  brew install dosfstools mtools && pip3 install pillow
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

IMG="$WORK/chiavetta.img"
SRC="$WORK/originali"
OUT="$WORK/recuperati"
mkdir -p "$SRC"

echo "1. Genero 12 foto di prova"
python3 - "$SRC" <<'PY'
import os, random, sys
from PIL import Image
out = sys.argv[1]
random.seed(1)
for i in range(12):
    w, h = random.randint(400, 1600), random.randint(300, 1200)
    img = Image.effect_noise((w, h), random.randint(20, 90)).convert("RGB")
    img.save(os.path.join(out, f"FOTO_{i:03}.JPG"), quality=random.randint(70, 95))
for i in range(3):
    Image.effect_noise((300, 200), 50).convert("RGB").save(os.path.join(out, f"SCREEN_{i}.PNG"))
PY

echo "2. Creo un volume FAT32 da 64 MB e copio le foto"
dd if=/dev/zero of="$IMG" bs=1M count=64 status=none
mkfs.vfat -F 32 -n PROVA "$IMG" >/dev/null
mmd -i "$IMG" ::/DCIM
mcopy -i "$IMG" "$SRC"/* ::/DCIM/

echo "3. Cancello tutte le foto e riformatto il volume"
mdel -i "$IMG" '::/DCIM/*'
mkfs.vfat -F 32 -n VUOTA "$IMG" >/dev/null

echo "4. Deep Scan e recupero"
cargo run --quiet --release --manifest-path "$ROOT/Cargo.toml" -p sectora-cli -- \
  carve "$IMG" --out "$OUT"

echo "5. Confronto degli hash"
python3 - "$SRC" "$OUT" <<'PY'
import hashlib, os, sys
def hashes(d):
    res = {}
    for root, _, files in os.walk(d):
        for f in files:
            with open(os.path.join(root, f), "rb") as fh:
                res[hashlib.sha256(fh.read()).hexdigest()] = f
    return res
orig, rec = hashes(sys.argv[1]), hashes(sys.argv[2])
ok = [n for h, n in orig.items() if h in rec]
missing = [n for h, n in orig.items() if h not in rec]
print(f"Recuperati con hash identico: {len(ok)}/{len(orig)}")
if missing:
    print("Mancanti:", ", ".join(sorted(missing)))
    sys.exit(1)
PY
