# Sectora

[![CI](https://github.com/AntonioMoccia/sectora/actions/workflows/ci.yml/badge.svg)](https://github.com/AntonioMoccia/sectora/actions/workflows/ci.yml)

Software open source di recupero dati per Windows, macOS e Linux: app desktop e riga di comando.
Ritrova file cancellati, ricostruisce le cartelle anche dopo una formattazione e verifica che i file recuperati siano integri.
Licenza: [GPL-3.0-or-later](LICENSE).

> Stato: v1.0 in sviluppo. Quick Scan FAT12/16/32, exFAT e NTFS con ricostruzione delle cartelle
> (anche dopo una formattazione), Deep Scan per firma, app desktop Tauri + React.

## Prima di tutto

- **Non scrivere niente sul disco da cui vuoi recuperare i dati.** Ogni nuovo file può
  sovrascrivere quelli cancellati. Il programma apre la sorgente in sola lettura e rifiuta
  una destinazione sullo stesso disco.
- **SSD con TRIM** (Mac recenti, la maggior parte dei PC): i file cancellati vengono
  azzerati dal disco in pochi secondi o minuti. In quel caso nessun software li recupera.
  Chiavette USB, schede SD e hard disk meccanici danno risultati molto migliori.
- Se il disco è instabile, crea prima un'immagine (`sectora image`) e lavora sull'immagine.

## Cosa recupera

| Situazione | Come | Nomi e cartelle |
| --- | --- | --- |
| File o cartelle cancellati (FAT, exFAT, NTFS) | Quick Scan del file system | Originali, struttura ricreata identica |
| Disco formattato (formattazione rapida) | Ricostruzione: vecchia MFT (NTFS), vecchie directory (FAT/exFAT) | NTFS: albero completo. FAT/exFAT: sottocartelle e file con nome; le cartelle di primo livello diventano "Cartella ritrovata N" |
| File system danneggiato o sconosciuto | Deep Scan per firma | Nomi generati (`f<offset>.jpg`) |

Partizioni: MBR (anche logiche) e GPT. Formati del Deep Scan: JPEG, PNG, GIF, HEIC, MP4, MOV,
PDF, DOCX, XLSX, PPTX, ZIP.

Ogni file trovato ha uno stato. Per i file cancellati di formato noto (foto, video, PDF,
Office, ZIP) il contenuto viene anche verificato: lo stato mostrato è l'esito della verifica.

| Stato | Significato |
| --- | --- |
| Verificato | La struttura del file è completa e coerente: il recupero sarà quasi certamente integro |
| Danneggiato | Intestazione o struttura non valide (file frammentato o sovrascritto): si recupera la parte leggibile |
| Recuperabile | Cancellato, posizione dei dati nota dai metadati (NTFS, exFAT contiguo) |
| Probabile | Cancellato, si assume che i dati fossero contigui (FAT). Corretto quasi sempre su dischi poco frammentati |
| Sovrascritto | I cluster risultano riusati da altri file: il recupero sarà probabilmente danneggiato |
| Non supportato | NTFS compresso/cifrato o con metadati incompleti |

Limiti noti: APFS e HFS+ (Mac) non sono ancora supportati dal Quick Scan, solo dal Deep Scan;
il Deep Scan recupera solo file contigui; formattazione completa (con azzeramento) e TRIM rendono
il recupero impossibile.

## Sicurezza

- La sorgente è aperta in sola lettura; il motore non ha API di scrittura verso di essa.
- Prima di ogni recupero sorgente e destinazione vengono risolte fino ai dischi fisici
  (partizioni, volumi APFS, LVM, unità Windows). Se coincidono, o se la verifica non è
  possibile, il recupero viene bloccato **prima** di creare qualsiasi cartella.
- I settori illeggibili vengono riletti uno per uno: un'area danneggiata non fa perdere
  i file vicini.

## App desktop

```sh
cd apps/desktop
npm install
npx tauri dev          # sviluppo
npx tauri build        # installer in target/release/bundle
```

Per leggere un disco fisico servono i privilegi di amministratore (macOS/Linux: avviare con `sudo`,
Windows: "Esegui come amministratore"). Senza privilegi si possono aprire le immagini disco.

## Riga di comando

```sh
cargo build --release      # binario in target/release/sectora

sectora devices                                  # dischi e partizioni
sectora quick   <disco|immagine> --deleted       # file cancellati con nome e cartella
sectora quick   <disco|immagine> --rebuild       # + struttura precedente a una formattazione
sectora restore <disco|immagine> --out <cartella su un ALTRO disco> --deleted [--rebuild]
sectora scan    <disco|immagine> --types jpg,mp4 # Deep Scan per firma
sectora carve   <disco|immagine> --out <cartella>
sectora image   <disco> sd.img                   # copia bit per bit
```

| Sistema | Disco | Note |
| --- | --- | --- |
| macOS | `/dev/rdisk4` | `diskutil list` per trovarlo, `diskutil unmountDisk /dev/disk4`, poi `sudo sectora …` |
| Linux | `/dev/sdb` o `/dev/mmcblk0` | `lsblk` per trovarlo, poi `sudo sectora …` |
| Windows | `\\.\PhysicalDrive1` | terminale "Esegui come amministratore" |

## Struttura

```
crates/engine        motore: sorgenti in sola lettura, partizioni, FAT/exFAT/NTFS, carving, recupero
crates/cli           riga di comando `sectora`
apps/desktop         app Tauri 2 + React + TypeScript
scripts/fat-test.sh  chiavetta FAT32 cancellata e riformattata → Deep Scan
scripts/fs-test.py   FAT12/16/32, exFAT, NTFS, disco MBR, dischi formattati → Quick Scan e ricostruzione
```

Regola di sicurezza: solo `crates/engine/src/source.rs` apre la sorgente, e non ha API di scrittura.

## Test

```sh
cargo test                       # unit test, immagini di riferimento, volumi corrotti, settori danneggiati
sudo python3 scripts/fs-test.py  # Linux: FAT/exFAT/NTFS reali, formattazione, frammentazione, sicurezza destinazione
                                 # (serve dosfstools, mtools, exfatprogs, exfat-fuse, ntfs-3g, pillow)
./scripts/fat-test.sh            # Deep Scan su FAT32 riformattato
```

## Roadmap

1. F0 Fondamenta ✅
2. F1 Deep Scan ✅
3. F2 File system: FAT/exFAT/NTFS, partizioni, ricostruzione dopo formattazione ✅
4. F3 App v1.0: interfaccia ✅ · verifica integrità ✅ · risultati paginati (milioni di file) ✅ ·
   installer firmati e notarizzati · helper privilegiato per i dischi ⏳
5. Dopo: HFS+/APFS, ext4, partizioni perse (TestDisk), riparazione video, supporto avviabile
