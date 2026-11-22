# Changelog

Le versioni seguono il [versionamento semantico](https://semver.org/lang/it/).

## [0.1.0] — 2026-11-22

Prima versione pubblica.

### Recupero
- Lettura dei file system FAT12/16/32, exFAT e NTFS, compresi i file e le cartelle cancellati, con
  nomi e struttura delle cartelle originali.
- Ricostruzione dopo una formattazione rapida: albero completo su NTFS; sottocartelle e file con
  nome su FAT ed exFAT.
- Ricerca per firma di 11 formati (JPEG, PNG, GIF, HEIC, MP4, MOV, PDF, DOCX, XLSX, PPTX, ZIP) con
  calcolo della lunghezza esatta del file.
- Partizioni MBR (anche logiche) e GPT.
- Verifica di integrità dei file recuperabili (Verificato / Danneggiato).
- Creazione di immagini disco; rilettura settore per settore delle aree illeggibili.

### Sicurezza
- Sorgente aperta solo in lettura.
- Recupero bloccato se la destinazione è sullo stesso disco fisico della sorgente (Windows, macOS
  con APFS, Linux con LVM), prima di creare qualsiasi cartella.

### Programmi
- App desktop (Tauri) per Windows, macOS e Linux, con risultati paginati per dischi con milioni di file.
- Riga di comando `sectora` con i comandi devices, quick, restore, scan, carve, image e types.
- Eseguibili pronti per Windows (x64, ARM64), macOS (universale) e Linux (x86_64, ARM64).

### Limiti noti
- Pacchetti non firmati.
- Su macOS i dischi fisici richiedono `sudo`.
- APFS, HFS+ ed ext4 solo con la ricerca per firma.

[0.1.0]: https://github.com/AntonioMoccia/sectora/releases/tag/v0.1.0
