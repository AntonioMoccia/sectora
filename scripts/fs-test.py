#!/usr/bin/env python3
"""Prova end-to-end del Quick Scan su file system reali.

Per ogni file system crea un volume con gli strumenti ufficiali, ci scrive
dei file, ne cancella una parte (anche un'intera cartella), poi esegue
`sectora restore --deleted` e confronta gli hash SHA-256 con gli originali.

Requisiti (Linux, root per i mount FUSE):
  sudo apt install dosfstools mtools exfatprogs exfat-fuse ntfs-3g
"""
import hashlib
import os
import random
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("SECTORA_BIN", os.path.join(ROOT, "target", "release", "sectora"))


def build_cli():
    """Compila la CLI e ritorna il percorso dell'eseguibile, come lo riporta Cargo
    (funziona anche con CARGO_TARGET_DIR personalizzato)."""
    import json as _json
    out = subprocess.run(
        ["cargo", "build", "--release", "-p", "sectora-cli", "--message-format=json-render-diagnostics"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout
    for line in out.splitlines():
        msg = _json.loads(line)
        if msg.get("reason") == "compiler-artifact" and msg.get("executable"):
            return msg["executable"]
    raise SystemExit("eseguibile sectora non trovato nell'output di cargo")
random.seed(7)


def run(*cmd, **kw):
    return subprocess.run(cmd, check=True, capture_output=True, text=True, **kw)


def payload(size):
    return random.randbytes(size)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def write_tree(base, files):
    for rel, data in files.items():
        p = os.path.join(base, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data)


def restored_hashes(out):
    res = {}
    for root, _, names in os.walk(out):
        for n in names:
            p = os.path.join(root, n)
            rel = os.path.relpath(p, out).split(os.sep, 1)[1]  # toglie la cartella del volume
            with open(p, "rb") as f:
                res[rel.replace(os.sep, "/")] = sha(f.read())
    return res


def first_char_unknown(rel):
    """FAT: un file cancellato senza nome lungo perde il primo carattere (diventa '_')."""
    d, _, n = rel.rpartition("/")
    return (d + "/" if d else "") + "_" + n[1:]


def check(name, image, deleted, work, fat=False):
    out = os.path.join(work, "out-" + name)
    run(BIN, "restore", image, "--out", out, "--deleted")
    got = restored_hashes(out)
    got_ci = {k.lower(): v for k, v in got.items()}
    ok, bad = 0, []
    for rel, data in deleted.items():
        h = got.get(rel)
        if h is None and fat:
            h = got_ci.get(first_char_unknown(rel).lower())
        if h == sha(data):
            ok += 1
        else:
            bad.append(rel + (" (diverso)" if h else " (mancante)"))
    status = "OK " if not bad else "ERR"
    print(f"[{status}] {name:<28} cancellati recuperati con hash identico: {ok}/{len(deleted)}")
    for b in bad:
        print(f"        {b}")
    return not bad


def sample_files(prefix=""):
    return {
        f"{prefix}DCIM/Vacanze al mare 2024/IMG_0001.jpg": payload(180_000),
        f"{prefix}DCIM/Vacanze al mare 2024/IMG_0002.jpg": payload(95_000),
        f"{prefix}Documenti/Relazione finale è pronta.docx": payload(42_000),
        f"{prefix}Documenti/piccolo.txt": b"testo breve\n" * 20,
        f"{prefix}Video/clip.mp4": payload(700_000),
        f"{prefix}tenere.pdf": payload(30_000),
    }


def fat(work, fat_bits, size_mb):
    name = f"FAT{fat_bits}"
    img = os.path.join(work, f"fat{fat_bits}.img")
    with open(img, "wb") as f:
        f.truncate(size_mb * 1024 * 1024)
    run("mkfs.vfat", "-F", str(fat_bits), "-n", "PROVA", img)
    src = os.path.join(work, f"src-fat{fat_bits}")
    files = sample_files()
    if fat_bits == 12:
        files = {k: v[:20_000] for k, v in files.items()}
    write_tree(src, files)
    for d in ["DCIM", "Documenti", "Video"]:
        run("mcopy", "-s", "-i", img, os.path.join(src, d), "::/")
    run("mcopy", "-i", img, os.path.join(src, "tenere.pdf"), "::/")
    # Cancella: due file singoli e un'intera cartella.
    run("mdel", "-i", img, "::/DCIM/Vacanze al mare 2024/IMG_0002.jpg")
    run("mdel", "-i", img, "::/Video/clip.mp4")
    run("mdeltree", "-i", img, "::/Documenti")
    deleted = {k: v for k, v in files.items() if "IMG_0002" in k or "clip" in k or k.startswith("Documenti/")}
    return check(name, img, deleted, work, fat=True)


def exfat(work):
    img = os.path.join(work, "exfat.img")
    with open(img, "wb") as f:
        f.truncate(64 * 1024 * 1024)
    run("mkfs.exfat", "-L", "SDCARD", img)
    loop = run("losetup", "-f", "--show", img).stdout.strip()
    mnt = os.path.join(work, "mnt-exfat")
    os.makedirs(mnt)
    files = sample_files()
    try:
        run("mount.exfat-fuse", loop, mnt)
        write_tree(mnt, files)
        os.remove(os.path.join(mnt, "DCIM/Vacanze al mare 2024/IMG_0002.jpg"))
        os.remove(os.path.join(mnt, "Video/clip.mp4"))
        shutil.rmtree(os.path.join(mnt, "Documenti"))
        run("umount", mnt)
    finally:
        subprocess.run(["umount", mnt], capture_output=True)
        run("losetup", "-d", loop)
    deleted = {k: v for k, v in files.items() if "IMG_0002" in k or "clip" in k or k.startswith("Documenti/")}
    return check("exFAT", img, deleted, work)


def ntfs_volume(work, img, size_mb):
    with open(img, "wb") as f:
        f.truncate(size_mb * 1024 * 1024)
    run("mkntfs", "-F", "-Q", "-q", "-L", "DATI", img)
    mnt = img + ".mnt"
    os.makedirs(mnt)
    files = sample_files()
    run("ntfs-3g", img, mnt)
    try:
        write_tree(mnt, files)
        # File frammentato: riempio, libero un buco, poi scrivo un file più grande del buco.
        fill = [os.path.join(mnt, f"riempi{i}.bin") for i in range(6)]
        for p in fill:
            with open(p, "wb") as f:
                f.write(payload(200_000))
        for p in fill[1::2]:
            os.remove(p)
        frag = payload(520_000)
        with open(os.path.join(mnt, "frammentato.bin"), "wb") as f:
            f.write(frag)
        files["frammentato.bin"] = frag
        os.remove(os.path.join(mnt, "DCIM/Vacanze al mare 2024/IMG_0002.jpg"))
        os.remove(os.path.join(mnt, "Video/clip.mp4"))
        os.remove(os.path.join(mnt, "frammentato.bin"))
        shutil.rmtree(os.path.join(mnt, "Documenti"))
        run("sync")
        time.sleep(0.5)
    finally:
        run("umount", mnt)
    deleted = {
        k: v
        for k, v in files.items()
        if "IMG_0002" in k or "clip" in k or k.startswith("Documenti/") or k == "frammentato.bin"
    }
    return deleted


def ntfs(work):
    img = os.path.join(work, "ntfs.img")
    deleted = ntfs_volume(work, img, 64)
    return check("NTFS (con file frammentato)", img, deleted, work)


def mbr_disk(work):
    """Disco con tabella MBR: partizione 1 FAT32, partizione 2 NTFS."""
    disk = os.path.join(work, "disco-mbr.img")
    p1 = os.path.join(work, "p1.img")
    p2 = os.path.join(work, "p2.img")
    with open(p1, "wb") as f:
        f.truncate(40 * 1024 * 1024)
    run("mkfs.vfat", "-F", "32", "-n", "FOTO", p1)
    src = os.path.join(work, "src-mbr")
    foto = {"Foto/estate.jpg": payload(150_000), "Foto/inverno.jpg": payload(60_000)}
    write_tree(src, foto)
    run("mcopy", "-s", "-i", p1, os.path.join(src, "Foto"), "::/")
    run("mdel", "-i", p1, "::/Foto/inverno.jpg")
    ntfs_deleted = ntfs_volume(work, p2, 64)

    start1, start2 = 2048, 2048 + 40 * 2048 + 2048
    with open(disk, "wb") as f:
        f.truncate((start2 + 64 * 2048 + 2048) * 512)
    mbr = bytearray(512)
    for slot, (kind, start, count) in enumerate([(0x0C, start1, 40 * 2048), (0x07, start2, 64 * 2048)]):
        o = 446 + slot * 16
        mbr[o + 4] = kind
        mbr[o + 8:o + 12] = start.to_bytes(4, "little")
        mbr[o + 12:o + 16] = count.to_bytes(4, "little")
    mbr[510:512] = b"\x55\xaa"
    with open(disk, "r+b") as d:
        d.write(mbr)
        for start, part in [(start1, p1), (start2, p2)]:
            d.seek(start * 512)
            with open(part, "rb") as f:
                shutil.copyfileobj(f, d)
    deleted = {"Foto/inverno.jpg": foto["Foto/inverno.jpg"], **ntfs_deleted}
    return check("Disco MBR (FAT32 + NTFS)", disk, deleted, work, fat=True)


def nested_files():
    return {
        "DCIM/100CANON/IMG_0101.jpg": payload(210_000),
        "DCIM/100CANON/IMG_0102.jpg": payload(160_000),
        "DCIM/101CANON/Compleanno nonna/IMG_0201.jpg": payload(120_000),
        "Documenti/Lavoro/Preventivo cliente.pdf": payload(64_000),
        "Documenti/Lavoro/Archivio 2024/fatture.xlsx": payload(48_000),
    }


def check_rebuilt(name, image, files, work, top_names_lost):
    """Dopo una formattazione: `restore --rebuild` deve ricostruire l'albero.

    Con top_names_lost (FAT/exFAT) i nomi delle cartelle di primo livello sono persi
    (stavano nella directory radice riscritta): si verifica il resto del percorso.
    """
    out = os.path.join(work, "out-" + name)
    run(BIN, "restore", image, "--out", out, "--rebuild")
    got = restored_hashes(out)
    ok, bad = 0, []
    for rel, data in files.items():
        if top_names_lost:
            tail = rel.split("/", 1)[1]
            match = [p for p in got if p.startswith("Struttura precedente/Cartella ritrovata ") and p.endswith("/" + tail)]
        else:
            match = [p for p in got if p == "Struttura precedente/" + rel]
        if any(got[m] == sha(data) for m in match):
            ok += 1
        else:
            bad.append(rel + (" (diverso)" if match else " (mancante)"))
    status = "OK " if not bad else "ERR"
    print(f"[{status}] {name:<28} albero ricostruito dopo formattazione: {ok}/{len(files)}")
    for b in bad:
        print(f"        {b}")
    return not bad


def fat32_formatted(work):
    img = os.path.join(work, "fat32-fmt.img")
    with open(img, "wb") as f:
        f.truncate(64 * 1024 * 1024)
    run("mkfs.vfat", "-F", "32", "-n", "CAMERA", img)
    src = os.path.join(work, "src-fat32-fmt")
    files = nested_files()
    write_tree(src, files)
    run("mcopy", "-s", "-i", img, os.path.join(src, "DCIM"), os.path.join(src, "Documenti"), "::/")
    run("mkfs.vfat", "-F", "32", "-n", "CAMERA", img)  # formattazione rapida
    return check_rebuilt("FAT32 formattato", img, files, work, True)


def exfat_formatted(work):
    img = os.path.join(work, "exfat-fmt.img")
    with open(img, "wb") as f:
        f.truncate(64 * 1024 * 1024)
    run("mkfs.exfat", "-L", "SDCARD", img)
    loop = run("losetup", "-f", "--show", img).stdout.strip()
    mnt = os.path.join(work, "mnt-exfat-fmt")
    os.makedirs(mnt)
    files = nested_files()
    try:
        run("mount.exfat-fuse", loop, mnt)
        write_tree(mnt, files)
        run("umount", mnt)
    finally:
        subprocess.run(["umount", mnt], capture_output=True)
        run("losetup", "-d", loop)
    run("mkfs.exfat", "-L", "SDCARD", img)
    return check_rebuilt("exFAT formattato", img, files, work, True)


def ntfs_formatted(work):
    img = os.path.join(work, "ntfs-fmt.img")
    with open(img, "wb") as f:
        f.truncate(96 * 1024 * 1024)
    run("mkntfs", "-F", "-Q", "-q", "-L", "DATI", img)
    mnt = img + ".mnt"
    os.makedirs(mnt)
    files = nested_files()
    run("ntfs-3g", img, mnt)
    try:
        # Abbastanza file da far crescere la MFT oltre i record che la nuova riscrive.
        for i in range(60):
            files[f"Varie/nota_{i:02}.txt"] = payload(3000 + i)
        write_tree(mnt, files)
    finally:
        run("umount", mnt)
    run("mkntfs", "-F", "-Q", "-q", "-L", "DATI", img)
    return check_rebuilt("NTFS formattato", img, files, work, False)


def fat32_fragmented(work):
    """Volume pieno, buchi alterni, file grande scritto a pezzi e poi cancellato.

    La verifica di integrità deve marcare come danneggiati i file sovrascritti dai
    frammenti e il file frammentato, e come verificati quelli rimasti intatti.
    """
    from PIL import Image
    import io, json

    def jpeg(w, h, q):
        buf = io.BytesIO()
        Image.effect_noise((w, h), 60).convert("RGB").save(buf, "JPEG", quality=q)
        return buf.getvalue()

    img = os.path.join(work, "fat32-frag.img")
    with open(img, "wb") as f:
        f.truncate(18 * 1024 * 1024)
    subprocess.run(["mkfs.vfat", "-F", "32", "-s", "8", img], capture_output=True, check=True)
    src = os.path.join(work, "src-frag")
    os.makedirs(src)
    for i in range(28):
        p = os.path.join(src, f"F{i:02}.JPG")
        with open(p, "wb") as f:
            f.write(jpeg(900, 700, 95))
        if subprocess.run(["mcopy", "-i", img, p, f"::/F{i:02}.JPG"], capture_output=True).returncode:
            break  # volume pieno
    holes = ["01", "05", "09", "13", "17"]
    for h in holes:
        run("mdel", "-i", img, f"::/F{h}.JPG")
    big = os.path.join(src, "grande.jpg")
    with open(big, "wb") as f:
        f.write(jpeg(2600, 1900, 95))
    run("mcopy", "-i", img, big, "::/GRANDE.JPG")
    run("mdel", "-i", img, "::/GRANDE.JPG")
    run("mdel", "-i", img, "::/F21.JPG")  # cancellato ma mai sovrascritto

    vols = json.loads(run(BIN, "quick", img, "--json").stdout)
    got = {e["name"]: e["integrity"] for v in vols for e in v["entries"] if e["deleted"]}
    # La voce di F01 viene riusata da GRANDE.JPG (primo posto libero nella directory).
    expect = {f"_{h}.JPG": "damaged" for h in holes[1:]}
    expect["_RANDE.JPG"] = "damaged"
    expect["_21.JPG"] = "valid"
    bad = [f"{k}: atteso {v}, trovato {got.get(k)}" for k, v in expect.items() if got.get(k) != v]
    ok = len(expect) - len(bad)
    status = "OK " if not bad else "ERR"
    print(f"[{status}] {'FAT32 frammentato':<28} integrità riconosciuta: {ok}/{len(expect)}")
    for b in bad:
        print(f"        {b}")
    return not bad


def destination_on_source(work):
    """Recupero verso una cartella del disco sorgente montato: deve essere rifiutato
    e non deve scrivere nulla (nemmeno creare la cartella di destinazione)."""
    img = os.path.join(work, "sorgente.img")
    with open(img, "wb") as f:
        f.truncate(32 * 1024 * 1024)
    run("mkntfs", "-F", "-Q", "-q", img)
    loop = run("losetup", "-f", "--show", img).stdout.strip()
    mnt = os.path.join(work, "mnt-sorgente")
    os.makedirs(mnt)
    try:
        run("ntfs-3g", loop, mnt)
        before = sha(open(img, "rb").read())
        r = subprocess.run([BIN, "restore", loop, "--out", os.path.join(mnt, "recuperati")], capture_output=True, text=True)
        created = os.path.exists(os.path.join(mnt, "recuperati"))
        run("umount", mnt)
        after = sha(open(img, "rb").read())
    finally:
        subprocess.run(["umount", mnt], capture_output=True)
        run("losetup", "-d", loop)
    ok = r.returncode != 0 and "stesso disco" in r.stderr and not created and before == after
    print(f"[{'OK ' if ok else 'ERR'}] {'Destinazione sul disco sorgente':<28} rifiutata senza scritture: {'sì' if ok else 'NO'}")
    if not ok:
        print("        ", r.stderr.strip()[:200], "cartella creata" if created else "", "immagine modificata" if before != after else "")
    return ok


def main():
    global BIN
    if "SECTORA_BIN" not in os.environ:
        BIN = build_cli()
    print(f"Binario: {BIN}")
    work = tempfile.mkdtemp()
    results = []
    try:
        results.append(fat(work, 12, 4))
        results.append(fat(work, 16, 32))
        results.append(fat(work, 32, 64))
        results.append(exfat(work))
        results.append(ntfs(work))
        results.append(mbr_disk(work))
        results.append(fat32_formatted(work))
        results.append(exfat_formatted(work))
        results.append(ntfs_formatted(work))
        results.append(fat32_fragmented(work))
        results.append(destination_on_source(work))
    finally:
        shutil.rmtree(work, ignore_errors=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
