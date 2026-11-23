# Changelog

Le versioni seguono il [versionamento semantico](https://semver.org/lang/it/).

## [0.1.2] — 2026-11-23

### Diagnosi
- Nuovo comando `sectora meta <disco> <file>`: salva solo le strutture del file system (nomi,
  dimensioni, cartelle; nessun contenuto dei file), da allegare a una segnalazione.
  `scripts/meta-to-img.py` ne ricostruisce un'immagine "vuota" su cui riprodurre la scansione.

### Recupero
- Le directory ricostruite scartano le voci impossibili (caratteri vietati, cluster o dimensioni
  fuori dal volume): niente più cartelle piene di nomi illeggibili lette da blocchi di dati.
- Le "Cartella ritrovata" senza nessun file vero vengono scartate.
- I file `._nome` creati dal Mac non vengono più controllati come se fossero del formato
  dell'estensione (prima risultavano "Danneggiato") e non contano nella descrizione delle cartelle.

## [0.1.1] — 2026-11-23

### Recupero dopo una formattazione (FAT32 ed exFAT)
- Le cartelle con molti file non si spezzano più in tante "Cartella ritrovata": i blocchi
  successivi del loro elenco, sparsi sul disco dopo l'azzeramento della FAT, vengono riattaccati
  alla cartella giusta (nomi spezzati tra due blocchi ricomposti con il loro checksum).
- FAT32: prima i file oltre il primo blocco di una cartella grande andavano persi; ora tornano tutti.
- Le cartelle di primo livello elencate nei blocchi sopravvissuti della vecchia radice tornano con il
  loro nome.
- Le cartelle senza nome hanno un nome che ne descrive il contenuto:
  `Cartella ritrovata 2 (48 foto, 3 video, 2024)`.

### App
- Le cartelle si possono rinominare prima del recupero (✎ o doppio clic): il recupero ricrea la
  struttura con i nuovi nomi.

### Test
- Nuove prove su file system reali: FAT32 ed exFAT formattati con cartelle da 120 file e una radice
  con 60 cartelle; ogni file deve tornare identico e nella sua cartella.

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

[0.1.2]: https://github.com/AntonioMoccia/sectora/releases/tag/v0.1.2
[0.1.1]: https://github.com/AntonioMoccia/sectora/releases/tag/v0.1.1
[0.1.0]: https://github.com/AntonioMoccia/sectora/releases/tag/v0.1.0
