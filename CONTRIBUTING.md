# Contribuire a Sectora

Grazie dell'interesse. Segnalazioni, test su dischi reali e pull request sono benvenuti.

## Segnalare un problema

Apri una [issue](https://github.com/AntonioMoccia/sectora/issues) indicando:

- sistema operativo e versione di Sectora (`sectora --version`);
- tipo di supporto (chiavetta, scheda SD, hard disk, SSD) e file system (FAT32, exFAT, NTFS…);
- cosa è successo ai dati (cancellati, formattati, disco danneggiato);
- comando eseguito e output completo.

Non allegare immagini disco con dati personali. Se serve un campione, crealo con dati di prova
(vedi "Provarlo senza rischi" nel README).

## Preparare l'ambiente

Segui "Compilare dai sorgenti" nel [README](README.md#compilare-dai-sorgenti) per il tuo sistema.

## Prima di aprire una pull request

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps/desktop && npm run build        # se hai toccato l'interfaccia
sudo python3 scripts/fs-test.py         # Linux, se hai toccato i file system
```

La CI esegue gli stessi controlli su Windows, macOS e Linux.

## Regole del progetto

1. **Mai scrivere sulla sorgente.** Solo `crates/engine/src/source.rs` apre il disco da recuperare e
   non espone API di scrittura. Una modifica che aggiunge scritture alla sorgente non viene accettata.
2. **La destinazione si verifica prima di creare qualsiasi file** (`check_destination`).
3. **Niente panic su dati corrotti.** Ogni valore letto dal disco va controllato; aggiungi un caso in
   `crates/engine/tests/corrupted_volumes.rs` per ogni nuovo parser.
4. **Ogni funzione nuova ha un test**, possibilmente su un file system reale in `scripts/fs-test.py`.
5. Testi dell'interfaccia e messaggi di errore in italiano, chiari per chi non è tecnico.

## Licenza

Inviando un contributo accetti che venga distribuito con la licenza del progetto,
[GPL-3.0-or-later](LICENSE).
