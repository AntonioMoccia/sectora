# Sectora

[![CI](https://github.com/AntonioMoccia/sectora/actions/workflows/ci.yml/badge.svg)](https://github.com/AntonioMoccia/sectora/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/AntonioMoccia/sectora)](https://github.com/AntonioMoccia/sectora/releases/latest)
[![Licenza: GPL-3.0](https://img.shields.io/badge/licenza-GPL--3.0-blue)](LICENSE)

Sectora recupera i file cancellati da chiavette USB, schede SD e dischi, ricostruisce le cartelle
anche dopo una formattazione e controlla che i file recuperati siano integri.
Funziona su **Windows, macOS e Linux**, con un'app grafica e con la riga di comando. È software libero.

- [Prima di tutto](#prima-di-tutto)
- [Installazione](#installazione): [Windows](#windows) · [macOS](#macos) · [Linux](#linux)
- [Usare l'app](#usare-lapp)
- [Usare la riga di comando](#usare-la-riga-di-comando)
- [Provarlo senza rischi](#provarlo-senza-rischi-con-una-chiavetta-finta)
- [Compilare dai sorgenti](#compilare-dai-sorgenti)
- [Risoluzione dei problemi](#risoluzione-dei-problemi)
- [Cosa recupera](#cosa-recupera) · [Sicurezza](#sicurezza) · [Per sviluppatori](#per-sviluppatori)

---

## Prima di tutto

1. **Smetti di usare il disco da cui hai perso i file.** Ogni file nuovo può sovrascrivere quelli
   cancellati. Non installare Sectora su quel disco e non salvarci i file recuperati.
2. **Gli SSD interni dei computer (tutti i Mac recenti e la maggior parte dei PC) azzerano i file
   cancellati** entro pochi secondi o minuti (TRIM). Da lì nessun programma può recuperarli.
   Chiavette, schede SD e hard disk meccanici invece danno buoni risultati.
3. **Se il disco fa rumori strani o si disconnette**, crea prima un'immagine (una copia completa) e
   lavora sulla copia: vedi `sectora image` più sotto.

---

## Installazione

Scarica il file giusto dalla [pagina delle release](https://github.com/AntonioMoccia/sectora/releases/latest).

| Sistema | App grafica (consigliata) | Solo riga di comando |
| --- | --- | --- |
| Windows 10 / 11 | `Sectora_…_x64-setup.exe` (o `.msi`) | `sectora-cli-…-windows-x86_64.zip` |
| Windows su ARM | `Sectora_…_x64-setup.exe` (in emulazione) | `sectora-cli-…-windows-arm64.zip` |
| macOS (Apple Silicon e Intel) | `Sectora_…_universal.dmg` | `sectora-cli-…-macos-universal.tar.gz` |
| Linux, qualsiasi distribuzione | `Sectora_…_amd64.AppImage` | `sectora-cli-…-linux-x86_64.tar.gz` |
| Debian / Ubuntu | `Sectora_…_amd64.deb` | come sopra |
| Fedora / openSUSE | `Sectora-…x86_64.rpm` | come sopra |
| Linux ARM 64 bit (Raspberry Pi 4/5) | — | `sectora-cli-…-linux-arm64.tar.gz` |

`SHA256SUMS.txt` contiene le impronte per controllare che il download sia integro.

> I pacchetti **non sono ancora firmati** con un certificato, quindi al primo avvio Windows e macOS
> mostrano un avviso. Qui sotto trovi come aprirli comunque.

### Windows

**App**

1. Scarica `Sectora_…_x64-setup.exe` e aprilo.
2. Se compare *"Windows ha protetto il PC"* (SmartScreen), fai clic su **Ulteriori informazioni → Esegui comunque**.
3. Completa l'installazione e avvia **Sectora** dal menu Start.
4. All'avvio Windows chiede i permessi di amministratore (finestra UAC): rispondi **Sì**.
   Servono per leggere dischi e chiavette. Senza quei permessi puoi aprire solo immagini disco.

**Riga di comando**

1. Scarica `sectora-cli-…-windows-x86_64.zip` ed estrailo, per esempio in `C:\Sectora`.
2. Apri il **Prompt dei comandi come amministratore** (Start → digita `cmd` → tasto destro →
   *Esegui come amministratore*).
3. Prova:
   ```bat
   cd C:\Sectora\sectora-cli-v0.1.0-windows-x86_64
   sectora.exe devices
   ```

### macOS

**App**

1. Scarica `Sectora_…_universal.dmg`, aprilo e trascina **Sectora** in **Applicazioni**.
2. Al primo avvio macOS blocca l'app perché non è firmata. Apri il Terminale e scrivi:
   ```sh
   xattr -dr com.apple.quarantine /Applications/Sectora.app
   ```
   In alternativa: **Impostazioni di Sistema → Privacy e sicurezza → "Apri comunque"**.
3. **Dischi e chiavette:** macOS permette di leggerli solo all'amministratore. Hai due strade:
   - **consigliata:** crea un'immagine della chiavetta con la riga di comando (vedi sotto) e aprila
     nell'app con *"Apri un'immagine disco…"*;
   - avvia l'app come amministratore dal Terminale:
     ```sh
     sudo /Applications/Sectora.app/Contents/MacOS/sectora-desktop
     ```

**Riga di comando**

```sh
cd ~/Downloads
tar xzf sectora-cli-v0.1.0-macos-universal.tar.gz
xattr -d com.apple.quarantine sectora-cli-v0.1.0-macos-universal/sectora
sudo mkdir -p /usr/local/bin
sudo mv sectora-cli-v0.1.0-macos-universal/sectora /usr/local/bin/
sectora --help
```

### Linux

**App (AppImage, qualsiasi distribuzione)**

```sh
chmod +x Sectora_*_amd64.AppImage
sudo ./Sectora_*_amd64.AppImage    # sudo serve per leggere i dischi
```

**App (Debian / Ubuntu)**

```sh
sudo apt install ./Sectora_*_amd64.deb
sudo sectora-desktop
```

**App (Fedora / openSUSE)**

```sh
sudo dnf install ./Sectora-*.x86_64.rpm
sudo sectora-desktop
```

Se l'app avviata con `sudo` non si apre (succede con alcuni desktop Wayland), avviala normalmente e
apri un'immagine creata con la riga di comando.

**Riga di comando** (binario statico: funziona su qualsiasi distribuzione)

```sh
tar xzf sectora-cli-v0.1.0-linux-x86_64.tar.gz
sudo install sectora-cli-v0.1.0-linux-x86_64/sectora /usr/local/bin/
sectora --help
```

---

## Usare l'app

1. **Scegli il disco.** Le chiavette sono segnate come *rimovibili*; sugli SSD compare un avviso.
   In alternativa apri un'immagine disco (`.img`).
2. **Scegli la scansione:**
   - **Completa (consigliata):** legge il file system, ricostruisce le cartelle anche dopo una
     formattazione e cerca i file per firma su tutto il disco;
   - **Rapida:** dura pochi secondi e mostra nomi e cartelle originali, ma non trova nulla dopo una
     formattazione;
   - **Profonda:** solo ricerca per firma. Funziona su dischi danneggiati, ma i file non hanno il nome originale.
3. **Scegli i file.** A sinistra ci sono le cartelle, in alto le categorie (Foto, Video, Documenti…),
   a destra l'anteprima. Ogni file ha uno stato: *Verificato* e *Recuperabile* sono i più affidabili.
4. **Recupera** in una cartella **su un altro disco**. Sectora ricrea la struttura delle cartelle.

---

## Usare la riga di comando

| Comando | A cosa serve |
| --- | --- |
| `sectora devices` | Elenca dischi e partizioni |
| `sectora quick <disco> --deleted` | File cancellati con nome e cartella originali |
| `sectora quick <disco> --rebuild` | In più ricostruisce le cartelle precedenti a una formattazione |
| `sectora restore <disco> --out <cartella> --deleted` | Recupera i file cancellati ricreando le cartelle |
| `sectora scan <disco> --types jpg,mp4` | Ricerca per firma (anche su dischi formattati o danneggiati) |
| `sectora carve <disco> --out <cartella>` | Ricerca per firma e recupero |
| `sectora image <disco> copia.img` | Copia bit per bit del disco, su cui poi lavorare |
| `sectora types` | Formati riconosciuti dalla ricerca per firma |

Opzioni di `quick` e `restore`: `--path <testo>` filtra per percorso, `--all` include anche i file
sovrascritti, `--rebuild` ricostruisce dopo una formattazione. `quick`, `scan` e `devices` accettano
`--json` per un output leggibile da altri programmi. `sectora <comando> --help` mostra tutte le opzioni.

### Come si chiama il disco

| Sistema | Come trovarlo | Esempio | Note |
| --- | --- | --- | --- |
| Windows | `sectora devices` | `\\.\PhysicalDrive1` | Prompt come amministratore. `PhysicalDrive0` è di solito il disco di Windows |
| macOS | `diskutil list` | `/dev/rdisk4` | Prima smonta: `diskutil unmountDisk /dev/disk4`. Usa `rdisk` (più veloce) con `sudo` |
| Linux | `lsblk` | `/dev/sdb`, `/dev/mmcblk0` | Con `sudo`. Usa il disco intero (`sdb`), non la partizione, per vedere tutto |

### Esempio completo: scheda SD con foto cancellate

**macOS**
```sh
diskutil list                                    # trova la scheda, es. disk4
diskutil unmountDisk /dev/disk4
sudo sectora image /dev/rdisk4 ~/Desktop/sd.img  # copia completa, una volta sola
sectora quick ~/Desktop/sd.img --deleted         # sull'immagine non serve sudo
sectora restore ~/Desktop/sd.img --out ~/Desktop/recuperati --deleted --rebuild
```

**Windows** (Prompt come amministratore)
```bat
sectora devices
sectora image \\.\PhysicalDrive1 C:\Recupero\sd.img
sectora restore C:\Recupero\sd.img --out C:\Recupero\file --deleted --rebuild
```

**Linux**
```sh
lsblk
sudo sectora image /dev/sdb ~/sd.img
sectora restore ~/sd.img --out ~/recuperati --deleted --rebuild
```

---

## Provarlo senza rischi con una chiavetta finta

Il modo migliore per prendere confidenza: crei un disco virtuale, ci copi delle foto, le cancelli e
poi le recuperi.

**macOS** (immagine `.dmg` vista come un disco vero)
```sh
hdiutil create -size 512m -fs ExFAT -volname PROVA ~/Desktop/prova.dmg
hdiutil attach ~/Desktop/prova.dmg
mkdir -p /Volumes/PROVA/DCIM && cp ~/Pictures/*.jpg /Volumes/PROVA/DCIM/
rm -r /Volumes/PROVA/DCIM && hdiutil detach /Volumes/PROVA
hdiutil attach -nomount ~/Desktop/prova.dmg      # stampa /dev/diskN
sudo sectora quick /dev/rdiskN --deleted
```

**Windows** (disco virtuale `.vhdx`, Prompt come amministratore)
```bat
mkdir C:\Prova
diskpart
  create vdisk file="C:\Prova\prova.vhdx" maximum=1024 type=expandable
  attach vdisk
  create partition primary
  format fs=ntfs quick label=PROVA
  assign letter=P
  exit
mkdir P:\DCIM & copy %USERPROFILE%\Pictures\*.jpg P:\DCIM\
rmdir /s /q P:\DCIM
sectora devices
sectora quick \\.\PhysicalDrive1 --deleted
```
Il numero di `PhysicalDrive` è quello che `sectora devices` indica per il disco virtuale da 1 GB.

**Linux**
```sh
truncate -s 512M prova.img && mkfs.exfat -L PROVA prova.img
LOOP=$(sudo losetup -f --show prova.img)
sudo mount $LOOP /mnt && sudo cp ~/Immagini/*.jpg /mnt/ && sudo rm /mnt/*.jpg && sudo umount /mnt
sudo sectora quick $LOOP --deleted
sudo losetup -d $LOOP
```

Per provare la ricostruzione dopo una formattazione, riformatta il disco virtuale (macOS:
`diskutil eraseVolume ExFAT NUOVA /dev/diskNs1`; Windows: `format fs=ntfs quick` in diskpart; Linux:
`mkfs.exfat` di nuovo) e usa `sectora quick … --rebuild`.

---

## Compilare dai sorgenti

Servono **Rust** (1.80 o più recente) e, per l'app, **Node.js 20 o più recente**.

### Windows

1. Prompt dei comandi o PowerShell **come amministratore**:
   ```bat
   winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
   winget install Rustlang.Rustup
   winget install OpenJS.NodeJS.LTS
   winget install Git.Git
   ```
   Senza `winget`: installa a mano i [Build Tools per Visual Studio](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
   con *"Sviluppo di applicazioni desktop C++"*, poi [rustup](https://rustup.rs), [Node.js](https://nodejs.org) e [Git](https://git-scm.com).
2. **Chiudi e riapri il terminale**, poi controlla: `cargo --version` e `node --version`.
3. Compila la riga di comando:
   ```bat
   git clone https://github.com/AntonioMoccia/sectora
   cd sectora
   cargo build --release
   target\release\sectora.exe --help
   ```
4. App:
   ```bat
   cd apps\desktop
   npm install
   npx tauri dev
   ```
   Per gli installer (`.exe` e `.msi` in `target\release\bundle`): `npx tauri build`.

### macOS

```sh
xcode-select --install                       # compilatore Apple
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
brew install node                            # oppure dal sito nodejs.org

git clone https://github.com/AntonioMoccia/sectora ~/src/sectora
cd ~/src/sectora
cargo build --release                        # binario: target/release/sectora
cd apps/desktop && npm install && npx tauri dev
```

Tieni il progetto **fuori da Scrivania e Documenti** se sono sincronizzate con iCloud: iCloud
interferisce con `node_modules` e può corrompere i file scaricati (vedi Risoluzione dei problemi).

### Linux (Debian / Ubuntu)

```sh
sudo apt update
sudo apt install -y build-essential curl git libwebkit2gtk-4.1-dev libgtk-3-dev \
  librsvg2-dev libayatana-appindicator3-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh && source "$HOME/.cargo/env"
curl -fsSL https://deb.nodesource.com/setup_22.x | sudo -E bash - && sudo apt install -y nodejs

git clone https://github.com/AntonioMoccia/sectora && cd sectora
cargo build --release
cd apps/desktop && npm install && npx tauri dev
```

Fedora: `sudo dnf install webkit2gtk4.1-devel gtk3-devel librsvg2-devel libappindicator-gtk3-devel`.
Arch: `sudo pacman -S webkit2gtk-4.1 gtk3 librsvg libappindicator-gtk3 base-devel`.

> Ti serve solo la riga di comando? Bastano Rust e `cargo build --release -p sectora-cli`: non servono
> Node né le librerie grafiche.

---

## Risoluzione dei problemi

| Messaggio o sintomo | Causa | Soluzione |
| --- | --- | --- |
| `"cargo" non è riconosciuto come comando` / `failed to run 'cargo metadata': program not found` | Rust non installato, o terminale aperto prima dell'installazione | Installa Rust (vedi sopra), poi **chiudi e riapri** il terminale |
| `linker 'link.exe' not found` (Windows) | Mancano i Build Tools di Visual Studio | Installa *"Sviluppo di applicazioni desktop C++"* |
| `Cannot find native binding` / `cli.darwin-arm64.node … extends beyond end of file` | Download di npm interrotto o corrotto (spesso per colpa di iCloud) | `rm -rf node_modules && npm cache verify && npm install`; sposta il progetto fuori da iCloud |
| npm segnala `install-scripts` e poi Vite o esbuild non partono | npm 11+ blocca gli script di installazione | `npm install-scripts ls`, poi `npm install-scripts approve esbuild` |
| `Permesso negato` / `Permission denied` aprendo un disco | Servono i permessi di amministratore | Windows: Prompt come amministratore; macOS e Linux: `sudo` |
| `Resource busy` su macOS | Il disco è montato | `diskutil unmountDisk /dev/diskN` |
| `si trova sullo stesso disco fisico della sorgente` | Stai salvando sul disco da recuperare | Scegli una cartella su un altro disco: è una protezione voluta |
| Nessun file cancellato trovato | SSD con TRIM, o disco formattato | Usa la scansione **Completa** o `--rebuild`; sugli SSD il recupero spesso è impossibile |
| File con stato *Danneggiato* | Il file era frammentato o è stato sovrascritto | Viene recuperata la parte leggibile; prova anche la ricerca per firma |
| Windows: "Windows ha protetto il PC" | Pacchetto non firmato | *Ulteriori informazioni → Esegui comunque* |
| macOS: "impossibile aprire, lo sviluppatore non può essere verificato" | App non firmata | `xattr -dr com.apple.quarantine /Applications/Sectora.app` |

Se il problema non è qui, apri una [segnalazione](https://github.com/AntonioMoccia/sectora/issues)
indicando sistema operativo, comando eseguito e messaggio completo.

---

## Cosa recupera

| Situazione | Metodo | Nomi e cartelle |
| --- | --- | --- |
| File o cartelle cancellati (FAT12/16/32, exFAT, NTFS) | Lettura del file system | Originali, struttura ricreata identica |
| Disco formattato (formattazione rapida) | Ricostruzione dalle strutture precedenti | NTFS: albero completo. FAT/exFAT: sottocartelle e file con nome; le cartelle di primo livello diventano "Cartella ritrovata N" |
| File system danneggiato o sconosciuto | Ricerca per firma | Nomi generati (`f<posizione>.jpg`) |

Partizioni MBR (anche logiche) e GPT. La ricerca per firma riconosce JPEG, PNG, GIF, HEIC, MP4,
MOV, PDF, DOCX, XLSX, PPTX e ZIP.

| Stato | Significato |
| --- | --- |
| Verificato | Struttura del file completa e coerente: quasi certamente integro |
| Recuperabile | Cancellato, posizione dei dati nota dai metadati |
| Probabile | Cancellato, si assume che i dati fossero contigui (FAT); di solito è corretto |
| Danneggiato | Struttura non valida (frammentato o sovrascritto): si recupera la parte leggibile |
| Sovrascritto | Lo spazio risulta riusato da altri file |
| Non supportato | NTFS compresso o cifrato, o metadati incompleti |

**Limiti attuali:** APFS e HFS+ (dischi dei Mac) ed ext4 si leggono solo con la ricerca per firma;
la ricerca per firma recupera solo file contigui; formattazione completa (con azzeramento), TRIM e
dischi con guasti fisici rendono il recupero impossibile.

---

## Sicurezza

- Il disco da recuperare viene aperto **in sola lettura**: il motore non ha funzioni per scriverci.
- Prima di salvare, sorgente e destinazione vengono risolte fino ai **dischi fisici** (partizioni,
  volumi APFS, LVM, unità Windows). Se coincidono, o se non è possibile verificarlo, il recupero
  viene bloccato **prima** di creare qualsiasi cartella.
- I file recuperati non sovrascrivono mai file esistenti.
- I settori illeggibili vengono riletti uno per uno, così un'area danneggiata non fa perdere i file vicini.

---

## Per sviluppatori

### Struttura

```
crates/engine        motore: sorgenti in sola lettura, partizioni, FAT/exFAT/NTFS, ricerca per firma,
                     verifica di integrità, recupero
crates/cli           riga di comando `sectora`
apps/desktop         app Tauri 2 + React + TypeScript
scripts/fs-test.py   prove su file system reali (FAT, exFAT, NTFS, formattazione, frammentazione)
scripts/fat-test.sh  ricerca per firma su FAT32 cancellato e riformattato
```

Regola di sicurezza: solo `crates/engine/src/source.rs` apre il disco sorgente, e non ha API di scrittura.

### Test

```sh
cargo test                                     # unità, immagini di riferimento, volumi corrotti
sudo python3 scripts/fs-test.py                # Linux: file system reali
./scripts/fat-test.sh                          # ricerca per firma su FAT32
cargo test --release -p sectora-desktop -- --ignored --nocapture   # prestazioni con 1 milione di file
```

`fs-test.py` richiede `dosfstools mtools exfatprogs exfat-fuse ntfs-3g` e Pillow.

### Pubblicare una release

1. Aggiorna la versione in `Cargo.toml` (workspace), `apps/desktop/package.json` e
   `apps/desktop/src-tauri/tauri.conf.json`, poi il [CHANGELOG](CHANGELOG.md).
2. `git tag v0.2.0 && git push origin v0.2.0`
3. Il workflow **Release** compila la riga di comando e l'app per tutti i sistemi e li allega alla
   release su GitHub, insieme a `SHA256SUMS.txt`.

Vedi anche [CONTRIBUTING.md](CONTRIBUTING.md).

### Roadmap

- [x] Motore, ricerca per firma, partizioni, FAT/exFAT/NTFS, ricostruzione dopo formattazione
- [x] App desktop, verifica di integrità, dischi con milioni di file
- [x] Eseguibili per Windows, macOS e Linux
- [ ] Installer firmati e notarizzati
- [ ] Helper privilegiato su macOS (leggere i dischi senza `sudo`)
- [ ] HFS+ / APFS, ext4, partizioni perse, riparazione video, supporto avviabile
- [ ] Interfaccia in inglese

## Licenza

[GPL-3.0-or-later](LICENSE). Puoi usare, studiare, modificare e ridistribuire Sectora; le versioni
modificate che distribuisci devono restare libere, con la stessa licenza.
