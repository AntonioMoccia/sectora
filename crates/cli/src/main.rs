//! `sectora`: riga di comando del motore di recupero dati.
//!
//! Esempi:
//!   sectora devices
//!   sectora image /dev/rdisk4 ~/Desktop/sd.img
//!   sectora scan ~/Desktop/sd.img --types jpg,mp4
//!   sectora carve ~/Desktop/sd.img --out ~/Desktop/recuperati
//!   sectora quick ~/Desktop/sd.img --deleted
//!   sectora restore ~/Desktop/sd.img --out ~/Desktop/recuperati --deleted

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use sectora_engine::{
    carve, check_destination, create_image, devices, recover_entry, recover_file, BlockSource,
    CarveOptions, Condition, Event, FileEntry, FileKind, Flow, Found, Integrity, ReadOnlySource,
    ScanOptions, ScanPhase,
};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "sectora",
    version,
    about = "Recupero dati open source (GPL-3.0)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Elenca dischi e partizioni.
    Devices {
        /// Output JSON.
        #[arg(long)]
        json: bool,
    },
    /// Cerca file per firma (Deep Scan) senza recuperarli.
    Scan {
        /// Disco, partizione o immagine .img.
        source: PathBuf,
        #[command(flatten)]
        scan: ScanArgs,
        /// Output JSON (una riga per file trovato).
        #[arg(long)]
        json: bool,
    },
    /// Cerca e recupera i file in una cartella su un altro disco.
    Carve {
        source: PathBuf,
        /// Cartella di destinazione: deve stare su un disco diverso dalla sorgente.
        #[arg(long, short)]
        out: PathBuf,
        #[command(flatten)]
        scan: ScanArgs,
    },
    /// Crea un'immagine bit per bit della sorgente (consigliato prima del recupero).
    Image {
        source: PathBuf,
        /// File .img da creare (non viene mai sovrascritto).
        output: PathBuf,
    },
    /// Quick Scan: legge il file system (FAT, exFAT, NTFS) ed elenca i file, anche cancellati,
    /// con nome e cartella originali.
    Quick {
        source: PathBuf,
        #[command(flatten)]
        filter: EntryFilter,
        /// Output JSON.
        #[arg(long)]
        json: bool,
    },
    /// Recupera i file trovati dal Quick Scan mantenendo nomi e cartelle.
    Restore {
        source: PathBuf,
        /// Cartella di destinazione: deve stare su un disco diverso dalla sorgente.
        #[arg(long, short)]
        out: PathBuf,
        #[command(flatten)]
        filter: EntryFilter,
    },
    /// Salva solo le strutture del file system (nomi, dimensioni, cartelle; nessun
    /// contenuto dei file) in un file da allegare a una segnalazione.
    Meta {
        source: PathBuf,
        /// File da creare (non viene mai sovrascritto).
        output: PathBuf,
    },
    /// Tipi di file supportati dal Deep Scan.
    Types,
}

#[derive(clap::Args)]
struct EntryFilter {
    /// Solo i file cancellati.
    #[arg(long)]
    deleted: bool,
    /// Solo i percorsi che contengono questo testo (senza distinzione maiuscole/minuscole).
    #[arg(long)]
    path: Option<String>,
    /// Includi anche i file probabilmente sovrascritti o non supportati.
    #[arg(long)]
    all: bool,
    /// Dopo una formattazione: cerca su tutto il volume le strutture del file system
    /// precedente e ricostruisce l'albero delle cartelle (più lento, legge tutto il disco).
    #[arg(long)]
    rebuild: bool,
}

impl EntryFilter {
    fn keep(&self, e: &FileEntry) -> bool {
        if self.deleted && !e.deleted {
            return false;
        }
        if !self.all && matches!(e.condition, Condition::Overwritten | Condition::Unsupported) {
            return false;
        }
        match &self.path {
            Some(p) => e.path.to_lowercase().contains(&p.to_lowercase()),
            None => true,
        }
    }
}

#[derive(clap::Args)]
struct ScanArgs {
    /// Tipi da cercare, separati da virgola (es. jpg,png,mp4). Default: tutti.
    #[arg(long, value_delimiter = ',')]
    types: Vec<String>,
    /// Passo di ricerca in byte.
    #[arg(long, default_value_t = 512)]
    step: u64,
}

impl ScanArgs {
    fn options(&self) -> Result<CarveOptions> {
        let mut kinds = BTreeSet::new();
        for t in &self.types {
            match FileKind::from_extension(t) {
                Some(k) => {
                    kinds.insert(k);
                }
                None => bail!("tipo non supportato: {t} (vedi `sectora types`)"),
            }
        }
        Ok(CarveOptions {
            step: self.step,
            kinds,
            ..Default::default()
        })
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Errore: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Devices { json } => cmd_devices(json),
        Command::Scan { source, scan, json } => cmd_scan(source, scan, json),
        Command::Carve { source, out, scan } => cmd_carve(source, out, scan),
        Command::Image { source, output } => cmd_image(source, output),
        Command::Quick {
            source,
            filter,
            json,
        } => cmd_quick(source, filter, json),
        Command::Restore {
            source,
            out,
            filter,
        } => cmd_restore(source, out, filter),
        Command::Meta { source, output } => cmd_meta(source, output),
        Command::Types => {
            for k in FileKind::ALL {
                println!("{:<6} {}", k.extension(), k.category());
            }
            Ok(())
        }
    }
}

fn open(source: &PathBuf) -> Result<ReadOnlySource> {
    ReadOnlySource::open(source).with_context(|| {
        format!(
            "impossibile aprire {} in sola lettura (per un disco servono i privilegi di amministratore)",
            source.display()
        )
    })
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn cmd_devices(json: bool) -> Result<()> {
    let list = devices::list_devices();
    if json {
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    if list.is_empty() {
        println!("Nessun dispositivo trovato.");
    }
    for d in list {
        let size = d.size.map(human).unwrap_or_else(|| "?".into());
        let mut notes = Vec::new();
        if d.is_partition {
            notes.push("partizione");
        }
        if d.removable == Some(true) {
            notes.push("rimovibile");
        }
        if d.rotational == Some(false) {
            notes.push("SSD/flash: i file cancellati potrebbero essere già azzerati (TRIM)");
        }
        println!(
            "{:<24} {:>10}  {}  {}",
            d.path.display(),
            size,
            d.model.unwrap_or_default(),
            notes.join(", ")
        );
    }
    Ok(())
}

/// Stampa l'avanzamento al massimo una volta al secondo.
/// Durata leggibile: "2 h 15 min", "12 min", "45 s".
fn duration(secs: f64) -> String {
    let s = secs.round() as u64;
    if s < 60 {
        return format!("{s} s");
    }
    let m = (s + 30) / 60;
    if m < 60 {
        return format!("{m} min");
    }
    format!("{} h {:02} min", m / 60, m % 60)
}

/// `scan_all` con l'avanzamento di ogni fase sul terminale.
fn scan_all_progress(src: &ReadOnlySource, opts: ScanOptions) -> Vec<sectora_engine::Volume> {
    let mut progress = Progress::new();
    let mut phase = None;
    let mut verify_started = Instant::now();
    let mut verify_last = Instant::now() - Duration::from_secs(2);
    let volumes = sectora_engine::scan_all_with(src, opts, |p, pos, total| {
        if phase != Some(p) {
            if phase.is_some() {
                eprintln!();
            }
            phase = Some(p);
            match p {
                ScanPhase::Rebuild => eprintln!("Lettura completa del disco (ricostruzione delle cartelle):"),
                ScanPhase::Finalize => eprintln!(
                    "Unione delle cartelle ritrovate: su dischi grandi può richiedere qualche minuto…"
                ),
                ScanPhase::Verify => {
                    verify_started = Instant::now();
                    eprintln!("Verifica dei file cancellati:")
                }
            }
        }
        match p {
            ScanPhase::Rebuild => progress.update(pos, total, 0),
            ScanPhase::Verify
                if verify_last.elapsed() >= Duration::from_secs(1) || pos == total =>
            {
                verify_last = Instant::now();
                let rate = pos as f64 / verify_started.elapsed().as_secs_f64().max(0.001);
                let eta = if rate > 0.0 && pos < total {
                    format!(
                        "  circa {} alla fine",
                        duration((total - pos) as f64 / rate)
                    )
                } else {
                    String::new()
                };
                eprint!("\r{pos} / {total} file{eta}      ");
                let _ = std::io::stderr().flush();
            }
            _ => {}
        }
    });
    if phase.is_some() {
        eprintln!();
    }
    volumes
}

struct Progress {
    started: Instant,
    last: Instant,
}

impl Progress {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last: now - Duration::from_secs(2),
        }
    }

    fn update(&mut self, pos: u64, total: u64, found: usize) {
        if self.last.elapsed() < Duration::from_secs(1) && pos < total {
            return;
        }
        self.last = Instant::now();
        let secs = self.started.elapsed().as_secs_f64().max(0.001);
        let pct = if total > 0 {
            pos as f64 * 100.0 / total as f64
        } else {
            100.0
        };
        let rate = pos as f64 / secs;
        let eta = if rate > 0.0 && pos < total {
            format!(
                "  circa {} alla fine",
                duration((total - pos) as f64 / rate)
            )
        } else {
            String::new()
        };
        let found = if found > 0 {
            format!("  file trovati: {found}")
        } else {
            String::new()
        };
        eprint!(
            "\r{pct:5.1}%  {} / {}  {}/s{eta}{found}   ",
            human(pos),
            human(total),
            human(rate as u64)
        );
        let _ = std::io::stderr().flush();
    }
}

fn scan(src: &ReadOnlySource, opts: &CarveOptions, mut on_found: impl FnMut(&Found)) -> Vec<Found> {
    let mut progress = Progress::new();
    let mut count = 0usize;
    let found = carve(src, opts, |ev| {
        match ev {
            Event::Progress { position, total } => progress.update(*position, *total, count),
            Event::Found(f) => {
                count += 1;
                on_found(f);
            }
            Event::ReadError {
                offset,
                length,
                message,
            } => {
                eprintln!(
                    "\nArea illeggibile a {offset} ({}): {message}",
                    human(*length)
                );
            }
        }
        Flow::Continue
    });
    eprintln!();
    found
}

fn cmd_scan(source: PathBuf, args: ScanArgs, json: bool) -> Result<()> {
    let src = open(&source)?;
    let opts = args.options()?;
    eprintln!("Scansione di {} ({})", src.describe(), human(src.size()));
    let found = scan(&src, &opts, |f| {
        if json {
            if let Ok(line) = serde_json::to_string(f) {
                println!("{line}");
            }
        }
    });
    if !json {
        for f in &found {
            println!(
                "{:<5} {:>14}  {:>10}",
                f.kind.extension(),
                f.offset,
                human(f.length)
            );
        }
    }
    eprintln!("Trovati {} file.", found.len());
    Ok(())
}

fn cmd_carve(source: PathBuf, out: PathBuf, args: ScanArgs) -> Result<()> {
    let src = open(&source)?;
    check_destination(&src, &out)?;
    let opts = args.options()?;
    eprintln!(
        "Recupero da {} ({}) in {}",
        src.describe(),
        human(src.size()),
        out.display()
    );
    let mut recovered = 0usize;
    let mut failed = 0usize;
    let found = scan(&src, &opts, |f| match recover_file(&src, f, &out) {
        Ok(_) => recovered += 1,
        Err(e) => {
            failed += 1;
            eprintln!(
                "\nNon recuperato ({} a {}): {e}",
                f.kind.extension(),
                f.offset
            );
        }
    });
    eprintln!(
        "Trovati {} file, recuperati {recovered}, non recuperati {failed}. Cartella: {}",
        found.len(),
        out.display()
    );
    Ok(())
}

fn cmd_image(source: PathBuf, output: PathBuf) -> Result<()> {
    let src = open(&source)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    check_destination(&src, &parent)?;
    eprintln!(
        "Immagine di {} ({}) in {}",
        src.describe(),
        human(src.size()),
        output.display()
    );
    let mut progress = Progress::new();
    let report = create_image(&src, &output, |pos, total| progress.update(pos, total, 0))?;
    eprintln!();
    if report.bad_ranges.is_empty() {
        eprintln!("Completata: {} letti, nessun errore.", human(report.bytes));
    } else {
        let bad: u64 = report.bad_ranges.iter().map(|r| r.1).sum();
        eprintln!(
            "Completata con {} illeggibili in {} aree (riempite di zeri).",
            human(bad),
            report.bad_ranges.len()
        );
    }
    Ok(())
}

fn cmd_meta(source: PathBuf, output: PathBuf) -> Result<()> {
    let src = open(&source)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    check_destination(&src, &parent)?;
    eprintln!(
        "Strutture di {} ({}) in {}: solo nomi e cartelle, nessun contenuto dei file.",
        src.describe(),
        human(src.size()),
        output.display()
    );
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    let mut out = std::io::BufWriter::new(file);
    let mut progress = Progress::new();
    let report = sectora_engine::meta::dump_metadata(&src, &mut out, |pos, total| {
        progress.update(pos, total, 0)
    })?;
    eprintln!();
    eprintln!(
        "Completato: {} di strutture in {} blocchi{}.",
        human(report.bytes),
        report.records,
        if report.unreadable > 0 {
            format!(", {} illeggibili saltati", human(report.unreadable))
        } else {
            String::new()
        }
    );
    Ok(())
}

fn condition_label(c: Condition) -> &'static str {
    match c {
        Condition::Intact => "integro",
        Condition::Recoverable => "recuperabile",
        Condition::Probable => "probabile",
        Condition::Overwritten => "sovrascritto",
        Condition::Unsupported => "non supportato",
    }
}

fn cmd_quick(source: PathBuf, filter: EntryFilter, json: bool) -> Result<()> {
    let src = open(&source)?;
    let volumes = scan_all_progress(
        &src,
        ScanOptions {
            rebuild: filter.rebuild,
            verify: true,
        },
    );
    if json {
        println!("{}", serde_json::to_string_pretty(&volumes)?);
        return Ok(());
    }
    if volumes.is_empty() {
        println!(
            "Nessun volume trovato. Prova il Deep Scan: sectora scan {}",
            source.display()
        );
    }
    for vol in &volumes {
        println!(
            "== {}  offset {}  {}{}",
            vol.display_name(),
            vol.offset,
            human(vol.size),
            vol.partition_type
                .as_ref()
                .map(|t| format!("  [{t}]"))
                .unwrap_or_default()
        );
        if let Some(err) = &vol.error {
            println!("   {err}");
            continue;
        }
        let mut shown = 0;
        for e in vol.entries.iter().filter(|e| !e.is_dir && filter.keep(e)) {
            let check = match e.integrity {
                Integrity::Valid => "verificato",
                Integrity::Damaged => "DANNEGGIATO",
                Integrity::Unchecked => "",
            };
            println!(
                "   {:<14} {:<11} {:>10}  {}",
                if e.deleted {
                    condition_label(e.condition)
                } else {
                    "presente"
                },
                check,
                human(e.size),
                e.path
            );
            shown += 1;
        }
        let deleted = vol
            .entries
            .iter()
            .filter(|e| e.deleted && !e.is_dir)
            .count();
        println!("   {shown} file mostrati, {deleted} cancellati in totale");
    }
    Ok(())
}

fn cmd_restore(source: PathBuf, out: PathBuf, filter: EntryFilter) -> Result<()> {
    let src = open(&source)?;
    check_destination(&src, &out)?;
    let volumes = scan_all_progress(
        &src,
        ScanOptions {
            rebuild: filter.rebuild,
            verify: true,
        },
    );
    let (mut ok, mut failed) = (0usize, 0usize);
    for vol in &volumes {
        // Anche le cartelle (vuote comprese), così la struttura torna com'era.
        for e in vol.entries.iter().filter(|e| filter.keep(e)) {
            match recover_entry(&src, vol, e, &out) {
                Ok(_) if e.is_dir => {}
                Ok(_) => ok += 1,
                Err(err) => {
                    failed += 1;
                    eprintln!("Non recuperato {}: {err}", e.path);
                }
            }
        }
    }
    eprintln!(
        "Recuperati {ok} file, non recuperati {failed}. Cartella: {}",
        out.display()
    );
    Ok(())
}
