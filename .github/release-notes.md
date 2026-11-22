Sectora recupera file cancellati da chiavette USB, schede SD e dischi, ricostruisce le cartelle anche dopo una formattazione e controlla che i file recuperati siano integri. Software libero (GPL-3.0).

## Quale file scaricare

| Sistema | App con interfaccia grafica | Solo riga di comando |
| --- | --- | --- |
| Windows 10/11 (PC normali) | `Sectora_…_x64-setup.exe` oppure `Sectora_…_x64_en-US.msi` | `sectora-cli-…-windows-x86_64.zip` |
| Windows su ARM (Surface Pro X, Snapdragon) | `Sectora_…_x64-setup.exe` (funziona in emulazione) | `sectora-cli-…-windows-arm64.zip` |
| macOS (Apple Silicon e Intel) | `Sectora_…_universal.dmg` | `sectora-cli-…-macos-universal.tar.gz` |
| Linux (qualsiasi distribuzione) | `Sectora_…_amd64.AppImage` | `sectora-cli-…-linux-x86_64.tar.gz` |
| Linux Debian / Ubuntu | `Sectora_…_amd64.deb` | come sopra |
| Linux Fedora / openSUSE | `Sectora-…x86_64.rpm` | come sopra |
| Linux su ARM (Raspberry Pi 4/5 a 64 bit) | — | `sectora-cli-…-linux-arm64.tar.gz` |

`SHA256SUMS.txt` contiene le impronte per verificare i download.

## Prima di usarlo

- **Non salvare niente sul disco da recuperare.** Sectora lo legge in sola lettura e blocca il recupero verso lo stesso disco.
- **I pacchetti non sono ancora firmati.** Windows (SmartScreen) e macOS (Gatekeeper) mostreranno un avviso: il [README](https://github.com/AntonioMoccia/sectora#installazione) spiega come aprirli.
- Per leggere un disco fisico servono i permessi di amministratore. Su Windows l'app li chiede da sola; su macOS e Linux vedi il README.

Istruzioni complete per ogni sistema: [README](https://github.com/AntonioMoccia/sectora#readme).
