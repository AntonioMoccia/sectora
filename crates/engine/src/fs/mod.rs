//! Quick Scan: lettura dei file system (FAT, exFAT, NTFS) per ritrovare i
//! file cancellati con nome, cartella, dimensione e data originali.

pub mod exfat;
pub mod fat;
pub mod ntfs;
pub mod partition;

use crate::source::{read_exact_at, BlockSource};
use serde::Serialize;
use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FsType {
    Fat12,
    Fat16,
    Fat32,
    ExFat,
    Ntfs,
}

impl FsType {
    pub fn name(self) -> &'static str {
        match self {
            FsType::Fat12 => "FAT12",
            FsType::Fat16 => "FAT16",
            FsType::Fat32 => "FAT32",
            FsType::ExFat => "exFAT",
            FsType::Ntfs => "NTFS",
        }
    }
}

/// Un pezzo di dati del file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extent {
    /// `length` byte a `offset` dall'inizio del volume.
    Disk { offset: u64, length: u64 },
    /// Area mai scritta (file sparse): zeri.
    Sparse { length: u64 },
    /// Dati salvati dentro i metadati (file NTFS piccoli).
    Inline(Vec<u8>),
}

impl Extent {
    pub fn len(&self) -> u64 {
        match self {
            Extent::Disk { length, .. } | Extent::Sparse { length } => *length,
            Extent::Inline(v) => v.len() as u64,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Quanto è affidabile il recupero di un file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// File presente, non cancellato.
    Intact,
    /// Cancellato, ma la posizione dei dati è nota dai metadati (NTFS, exFAT contiguo).
    Recoverable,
    /// Cancellato: si assume che i dati siano contigui (FAT). Di solito corretto
    /// per file scritti su un disco poco frammentato.
    Probable,
    /// Cancellato e i suoi cluster risultano riusati da altri file.
    Overwritten,
    /// Dati non recuperabili da questo motore (compressi, cifrati, metadati mancanti).
    Unsupported,
}

/// Esito del controllo del contenuto di un file cancellato.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Integrity {
    /// Formato non verificabile (testo, formati non riconosciuti) o file non cancellato.
    #[default]
    Unchecked,
    /// La struttura del formato è completa e coerente: il file è quasi certamente integro.
    Valid,
    /// Intestazione o struttura non valide: dati frammentati o sovrascritti.
    Damaged,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    /// Identificatore unico nel volume.
    pub id: u64,
    /// Percorso completo con `/` come separatore, es. `/DCIM/100CANON/IMG_0001.JPG`.
    pub path: String,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub deleted: bool,
    /// Ultima modifica, secondi Unix (ora locale del dispositivo per FAT/exFAT).
    pub modified: Option<i64>,
    pub condition: Condition,
    pub integrity: Integrity,
    #[serde(skip)]
    pub extents: Vec<Extent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Volume {
    /// Indice della partizione (0 = disco senza tabella partizioni).
    pub index: u32,
    /// Offset del volume nella sorgente.
    pub offset: u64,
    pub size: u64,
    pub fs: Option<FsType>,
    pub label: Option<String>,
    pub partition_type: Option<String>,
    pub cluster_size: u32,
    /// Motivo per cui il volume non è stato letto (file system non supportato o danneggiato).
    pub error: Option<String>,
    pub entries: Vec<FileEntry>,
    /// Stato del file system usato dalla ricostruzione dopo formattazione.
    #[serde(skip)]
    pub(crate) state: Option<FsState>,
    /// Ricostruzione di un NTFS precedente su un volume oggi FAT o exFAT
    /// (penna formattata NTFS e poi riformattata con un altro file system).
    #[serde(skip)]
    pub(crate) foreign: Option<FsState>,
}

/// Dati del file system letto, che servono a ricostruire la struttura precedente
/// a una formattazione mentre il disco viene letto per intero.
#[derive(Clone)]
pub(crate) enum FsState {
    Fat(fat::State),
    ExFat(exfat::State),
    Ntfs(ntfs::State),
}

impl std::fmt::Debug for FsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FsState::Fat(_) => "FsState::Fat",
            FsState::ExFat(_) => "FsState::ExFat",
            FsState::Ntfs(_) => "FsState::Ntfs",
        })
    }
}

impl Volume {
    pub fn display_name(&self) -> String {
        let fs = self.fs.map(|f| f.name()).unwrap_or("sconosciuto");
        match &self.label {
            Some(l) => format!("{l} ({fs})"),
            None => format!("Volume {} ({fs})", self.index),
        }
    }
}

/// Vista di una porzione di una sorgente (una partizione).
pub struct SubSource<'a> {
    inner: &'a dyn BlockSource,
    offset: u64,
    len: u64,
}

impl<'a> SubSource<'a> {
    pub fn new(inner: &'a dyn BlockSource, offset: u64, len: u64) -> Self {
        let len = len.min(inner.size().saturating_sub(offset));
        Self { inner, offset, len }
    }
}

impl BlockSource for SubSource<'_> {
    fn size(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.len {
            return Ok(0);
        }
        let n = buf.len().min((self.len - offset) as usize);
        self.inner.read_at(self.offset + offset, &mut buf[..n])
    }
    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }
    fn describe(&self) -> String {
        format!("{}@{}", self.inner.describe(), self.offset)
    }
}

/// Riconosce il file system dal primo settore del volume.
pub fn detect(sector0: &[u8]) -> Option<FsType> {
    if sector0.len() < 512 {
        return None;
    }
    if &sector0[3..11] == b"NTFS    " {
        return Some(FsType::Ntfs);
    }
    if &sector0[3..11] == b"EXFAT   " {
        return Some(FsType::ExFat);
    }
    fat::detect(sector0)
}

/// Cartella in cui compare la struttura ricostruita dopo una formattazione.
pub const REBUILT_DIR: &str = "/Struttura precedente";

/// Cartella dell'albero ricostruito da un NTFS precedente su un volume che oggi
/// ha un altro file system.
pub const REBUILT_NTFS_DIR: &str = "/Struttura precedente (NTFS)";

/// Coda di una directory ricostruita (FAT/exFAT). L'elenco di una cartella con
/// molti file occupa più cluster, collegati dalla FAT: dopo una formattazione
/// la FAT è azzerata e i cluster successivi al primo restano sparsi sul disco.
/// Si annota quanti ne mancano e dove finiscono gli ultimi dati scritti, per
/// riattaccare i blocchi ritrovati alla cartella giusta.
#[derive(Clone, Debug)]
pub(crate) struct DirTail {
    pub path: String,
    /// Cluster dell'elenco ancora da trovare (FAT: 1 se l'elenco non è terminato).
    pub missing: u32,
    /// Ultimo cluster occupato dall'elenco o dai file che contiene.
    pub last_end: u32,
    /// exFAT: voci finali incomplete (un entry set a cavallo tra due cluster),
    /// da anteporre al blocco successivo.
    pub pending: Vec<u8>,
    /// Nome dell'ultimo file elencato.
    pub last_name: String,
}

impl DirTail {
    /// Coda di una directory appena letta: `last` è il suo ultimo cluster,
    /// `files` le voci elencate direttamente al suo interno.
    pub(crate) fn new(
        path: &str,
        last: u32,
        missing: u32,
        files: &[FileEntry],
        data_offset: u64,
        cluster_size: u64,
    ) -> Self {
        let mut last_end = last;
        for e in files.iter().filter(|e| !e.is_dir) {
            if let Some(Extent::Disk { offset, length }) = e.extents.last() {
                if *length > 0 && *offset >= data_offset {
                    let c = (offset + length - 1 - data_offset) / cluster_size + 2;
                    last_end = last_end.max(c.min(u32::MAX as u64) as u32);
                }
            }
        }
        DirTail {
            path: path.to_string(),
            missing,
            last_end,
            pending: Vec::new(),
            last_name: files
                .iter()
                .rev()
                .find(|e| !e.is_dir)
                .map(|e| e.name.to_lowercase())
                .unwrap_or_default(),
        }
    }
}

/// Cartella a cui appartiene il blocco di elenco orfano al cluster `frag`.
///
/// `fit` valuta quanto il blocco si accorda con la coda di una cartella:
/// 2 = la voce spezzata tra i due blocchi si ricompone (checksum corretto),
/// 1 = compatibile, 0 = incompatibile. A parità contano la somiglianza tra il
/// primo nome del blocco (`first_name`) e l'ultimo della cartella (i file di una
/// cartella hanno spesso nomi in sequenza: IMG_0042, IMG_0043), poi la cartella
/// scritta per ultima prima del blocco: Windows alloca il nuovo cluster di una
/// cartella quando l'elenco si riempie, subito dopo i dati dell'ultimo file.
pub(crate) fn fragment_owner(
    tails: &[DirTail],
    frag: u32,
    first_name: Option<&str>,
    fit: impl Fn(&DirTail) -> u8,
) -> Option<usize> {
    let first = first_name.map(str::to_lowercase);
    tails
        .iter()
        .enumerate()
        .filter(|(_, t)| t.missing > 0 && t.last_end < frag)
        .map(|(i, t)| {
            let similar = first.as_deref().map_or(0, |f| {
                f.chars()
                    .zip(t.last_name.chars())
                    .take_while(|(a, b)| a == b)
                    .count()
            });
            (fit(t), similar, t.last_end, i)
        })
        .filter(|(f, ..)| *f > 0)
        .max()
        .map(|(.., i)| i)
}

/// Opzioni del Quick Scan.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanOptions {
    /// Cerca su tutto il volume le strutture del file system precedente
    /// (record MFT, directory FAT/exFAT) sopravvissute a una formattazione e ne
    /// ricostruisce l'albero sotto [`REBUILT_DIR`]. Legge l'intero disco.
    pub rebuild: bool,
    /// Controlla il contenuto dei file cancellati ([`Integrity`]).
    pub verify: bool,
}

/// Quick Scan dell'intera sorgente: trova le partizioni e legge ogni volume.
pub fn quick_scan(src: &dyn BlockSource) -> Vec<Volume> {
    scan_all(src, ScanOptions::default())
}

/// Quick Scan con opzioni. Per seguire l'avanzamento della ricostruzione o
/// interromperla usare [`scan_volumes`], [`rebuild_pass`] e [`verify_volumes`].
pub fn scan_all(src: &dyn BlockSource, opts: ScanOptions) -> Vec<Volume> {
    let mut volumes = scan_volumes(src);
    if opts.rebuild {
        rebuild_pass(src, &mut volumes, |_, _| crate::carve::Flow::Continue);
    }
    if opts.verify {
        verify_volumes(src, &mut volumes, &|| false);
    }
    volumes
}

/// Legge le tabelle delle partizioni e il file system di ogni volume (veloce).
pub fn scan_volumes(src: &dyn BlockSource) -> Vec<Volume> {
    let mut sector0 = vec![0u8; 512];
    if read_exact_at(src, 0, &mut sector0).is_err() {
        return Vec::new();
    }
    // Disco senza tabella partizioni (chiavette "superfloppy") o immagine di una partizione.
    if detect(&sector0).is_some() {
        return vec![scan_volume(src, 0, src.size(), 0, None)];
    }
    partition::read_partitions(src)
        .into_iter()
        .map(|p| scan_volume(src, p.offset, p.size, p.index, Some(p.kind)))
        .collect()
}

/// Legge un singolo volume a `offset`.
pub fn scan_volume(
    src: &dyn BlockSource,
    offset: u64,
    size: u64,
    index: u32,
    partition_type: Option<String>,
) -> Volume {
    let view = SubSource::new(src, offset, size);
    let mut vol = Volume {
        index,
        offset,
        size: view.size(),
        fs: None,
        label: None,
        partition_type,
        cluster_size: 0,
        error: None,
        entries: Vec::new(),
        state: None,
        foreign: None,
    };
    let mut sector0 = vec![0u8; 512];
    if let Err(e) = read_exact_at(&view, 0, &mut sector0) {
        vol.error = Some(format!("primo settore illeggibile: {e}"));
        return vol;
    }
    vol.fs = detect(&sector0);
    let result = match vol.fs {
        Some(FsType::Ntfs) => ntfs::scan(&view, &mut vol),
        Some(FsType::ExFat) => exfat::scan(&view, &mut vol),
        Some(FsType::Fat12 | FsType::Fat16 | FsType::Fat32) => fat::scan(&view, &mut vol),
        None => {
            Err("file system non riconosciuto o non ancora supportato (usa il Deep Scan)".into())
        }
    };
    if let Err(e) = result {
        vol.error = Some(e);
    }
    if matches!(
        vol.fs,
        Some(FsType::Fat12 | FsType::Fat16 | FsType::Fat32 | FsType::ExFat)
    ) {
        vol.foreign = Some(FsState::Ntfs(ntfs::State::foreign()));
    }
    vol
}

/// Passa a ogni volume un blocco di dati letto dalla sorgente a `abs`, per la
/// ricostruzione. Da chiamare durante una lettura completa (es. il Deep Scan).
pub fn feed_rebuild(src: &dyn BlockSource, volumes: &mut [Volume], abs: u64, chunk: &[u8]) {
    let end = abs + chunk.len() as u64;
    for vol in volumes.iter_mut() {
        let vol_end = vol.offset + vol.size;
        if end <= vol.offset || abs >= vol_end {
            continue;
        }
        let from = abs.max(vol.offset);
        let to = end.min(vol_end);
        let slice = &chunk[(from - abs) as usize..(to - abs) as usize];
        let rel = from - vol.offset;
        let view = SubSource::new(src, vol.offset, vol.size);
        match vol.state.as_mut() {
            Some(FsState::Fat(s)) => s.feed(&view, rel, slice),
            Some(FsState::ExFat(s)) => s.feed(&view, rel, slice),
            Some(FsState::Ntfs(s)) => s.feed(&view, rel, slice),
            None => {}
        }
        if let Some(FsState::Ntfs(s)) = vol.foreign.as_mut() {
            s.feed(&view, rel, slice);
        }
    }
}

/// Conclude la ricostruzione: aggiunge ai volumi le voci ritrovate.
pub fn finish_rebuild(src: &dyn BlockSource, volumes: &mut [Volume]) {
    for vol in volumes.iter_mut() {
        let view = SubSource::new(src, vol.offset, vol.size);
        let found = match vol.state.take() {
            Some(FsState::Fat(s)) => s.finish(&view),
            Some(FsState::ExFat(s)) => s.finish(&view),
            Some(FsState::Ntfs(s)) => s.finish(&view),
            None => Vec::new(),
        };
        let mut found = found;
        describe_found_folders(&mut found);
        vol.entries.extend(found);
        if let Some(FsState::Ntfs(s)) = vol.foreign.take() {
            vol.entries.extend(s.finish(&view));
        }
    }
}

/// Anno (calendario civile) di un istante Unix.
fn year_of(ts: i64) -> i64 {
    let days = ts.div_euclid(86_400);
    // Algoritmo di H. Hinnant (civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(month <= 2)
}

/// Categoria di un file per il riassunto di una cartella ritrovata.
fn kind_of(name: &str) -> usize {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "heic" | "heif" | "gif" | "bmp" | "tif" | "tiff" | "webp"
        | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "orf" | "rw2" => 0,
        "mp4" | "mov" | "avi" | "mkv" | "m4v" | "3gp" | "mts" | "wmv" | "webm" => 1,
        "mp3" | "wav" | "m4a" | "flac" | "aac" | "ogg" | "wma" => 2,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "odt" | "ods"
        | "odp" | "rtf" | "csv" | "pages" | "numbers" | "key" => 3,
        _ => 4,
    }
}

/// Riassunto del contenuto: "48 foto, 3 video, 2019-2024" o "vuota".
fn folder_summary(counts: &[usize; 5], years: Option<(i64, i64)>) -> String {
    const LABELS: [(&str, &str); 5] = [
        ("foto", "foto"),
        ("video", "video"),
        ("audio", "audio"),
        ("documento", "documenti"),
        ("altro file", "altri file"),
    ];
    let mut kinds: Vec<(usize, usize)> = counts
        .iter()
        .enumerate()
        .filter(|(_, &n)| n > 0)
        .map(|(k, &n)| (n, k))
        .collect();
    if kinds.is_empty() {
        return "vuota".into();
    }
    kinds.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut parts: Vec<String> = kinds
        .iter()
        .take(3)
        .map(|&(n, k)| format!("{n} {}", if n == 1 { LABELS[k].0 } else { LABELS[k].1 }))
        .collect();
    if let Some((a, b)) = years {
        parts.push(if a == b {
            a.to_string()
        } else {
            format!("{a}-{b}")
        });
    }
    parts.join(", ")
}

/// Le cartelle di primo livello ritrovate senza nome ("Cartella ritrovata N")
/// prendono un nome che ne descrive il contenuto, per riconoscerle a colpo
/// d'occhio: "Cartella ritrovata 2 (48 foto, 3 video, 2024)".
pub(crate) fn describe_found_folders(entries: &mut Vec<FileEntry>) {
    use std::collections::HashMap;
    let base = format!("{REBUILT_DIR}/Cartella ritrovata ");
    let top_of = |path: &str| -> Option<String> {
        let rest = path.strip_prefix(&base)?;
        let n = rest.split('/').next()?;
        Some(format!("{base}{n}"))
    };
    /// File per categoria e intervallo degli anni.
    type Stats = ([usize; 5], Option<(i64, i64)>);
    let mut stats: HashMap<String, Stats> = HashMap::new();
    for e in entries.iter() {
        let Some(top) = top_of(&e.path) else { continue };
        let st = stats.entry(top).or_default();
        if e.is_dir || e.name.starts_with("._") {
            continue;
        }
        st.0[kind_of(&e.name)] += 1;
        if let Some(ts) = e.modified {
            let y = year_of(ts);
            if (1981..=2100).contains(&y) {
                st.1 = Some(st.1.map_or((y, y), |(a, b)| (a.min(y), b.max(y))));
            }
        }
    }
    // Cartelle senza nessun file (solo voci illeggibili o cartelle vuote): si scartano.
    let empty: std::collections::HashSet<String> = stats
        .iter()
        .filter(|(_, (counts, _))| counts.iter().sum::<usize>() == 0)
        .map(|(top, _)| top.clone())
        .collect();
    entries.retain(|e| top_of(&e.path).map_or(true, |t| !empty.contains(&t)));
    let renamed: HashMap<String, String> = stats
        .into_iter()
        .filter(|(top, _)| !empty.contains(top))
        .map(|(top, (counts, years))| {
            let new = format!("{top} ({})", folder_summary(&counts, years));
            (top, new)
        })
        .collect();
    for e in entries.iter_mut() {
        let Some(top) = top_of(&e.path) else { continue };
        let Some(new) = renamed.get(&top) else {
            continue;
        };
        e.path = format!("{new}{}", &e.path[top.len()..]);
        if e.path == *new {
            e.name = new.rsplit('/').next().unwrap_or_default().to_string();
        }
    }
}

/// Lettura completa della sorgente solo per la ricostruzione (senza Deep Scan).
pub fn rebuild_pass(
    src: &dyn BlockSource,
    volumes: &mut [Volume],
    mut on_progress: impl FnMut(u64, u64) -> crate::carve::Flow,
) {
    let opts = crate::carve::CarveOptions {
        detect: false,
        ..Default::default()
    };
    crate::carve::carve_with(
        src,
        &opts,
        |ev| match ev {
            crate::carve::Event::Progress { position, total } => on_progress(*position, *total),
            _ => crate::carve::Flow::Continue,
        },
        &mut |abs, chunk| feed_rebuild(src, volumes, abs, chunk),
    );
    finish_rebuild(src, volumes);
}

/// Sorgente che espone i dati di un file (i suoi extent) come un disco a sé.
pub struct EntrySource<'a> {
    src: &'a dyn BlockSource,
    base: u64,
    extents: &'a [Extent],
    size: u64,
}

impl<'a> EntrySource<'a> {
    pub fn new(src: &'a dyn BlockSource, vol: &Volume, entry: &'a FileEntry) -> Self {
        let size = entry
            .extents
            .iter()
            .map(|e| e.len())
            .sum::<u64>()
            .min(entry.size);
        Self {
            src,
            base: vol.offset,
            extents: &entry.extents,
            size,
        }
    }
}

impl BlockSource for EntrySource<'_> {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.size {
            return Ok(0);
        }
        let mut pos = 0u64;
        for ext in self.extents {
            let len = ext.len();
            if offset < pos + len {
                let inner = offset - pos;
                let n = (buf.len() as u64).min(len - inner).min(self.size - offset) as usize;
                return match ext {
                    Extent::Disk { offset: o, .. } => {
                        self.src.read_at(self.base + o + inner, &mut buf[..n])
                    }
                    Extent::Sparse { .. } => {
                        buf[..n].fill(0);
                        Ok(n)
                    }
                    Extent::Inline(v) => {
                        buf[..n].copy_from_slice(&v[inner as usize..inner as usize + n]);
                        Ok(n)
                    }
                };
            }
            pos += len;
        }
        Ok(0)
    }
    fn describe(&self) -> String {
        "file".into()
    }
}

/// Controlla il contenuto di un file: per i formati noti (foto, video, PDF,
/// Office, ZIP) l'intestazione deve corrispondere all'estensione e la struttura
/// interna deve essere completa entro la dimensione del file.
pub fn verify_entry(src: &dyn BlockSource, vol: &Volume, entry: &FileEntry) -> Integrity {
    use crate::carve::formats::{self, Family};
    use crate::carve::reader::ByteReader;
    if entry.is_dir || entry.size == 0 || entry.extents.is_empty() {
        return Integrity::Unchecked;
    }
    let ext = entry
        .name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let expected = crate::carve::FileKind::from_extension(&ext).map(Family::of);
    let data = EntrySource::new(src, vol, entry);
    let mut head = [0u8; 16];
    let n = data.read_at(0, &mut head).unwrap_or(0);
    let found = formats::detect(&head[..n]);
    match (expected, found) {
        (None, None) => Integrity::Unchecked,
        (Some(_), None) => Integrity::Damaged,
        (Some(e), Some(f)) if e != f => Integrity::Damaged,
        (_, Some(family)) => {
            // I file enormi si verificano solo dall'intestazione.
            if entry.size > 1 << 30 {
                return Integrity::Unchecked;
            }
            let mut r = ByteReader::new(&data);
            match formats::measure(&mut r, family, 0) {
                Some((len, _)) if len <= entry.size => Integrity::Valid,
                _ => Integrity::Damaged,
            }
        }
    }
}

/// Verifica tutti i file cancellati. `stop` viene controllato tra un file e l'altro.
pub fn verify_volumes(src: &dyn BlockSource, volumes: &mut [Volume], stop: &dyn Fn() -> bool) {
    for vol in volumes.iter_mut() {
        let snapshot = vol.clone_meta();
        for e in vol.entries.iter_mut() {
            if stop() {
                return;
            }
            // I file "._nome" sono metadati del Mac (AppleDouble), non il formato
            // indicato dall'estensione: non si controllano.
            if e.deleted
                && !e.is_dir
                && e.condition != Condition::Unsupported
                && !e.name.starts_with("._")
            {
                e.integrity = verify_entry(src, &snapshot, e);
            }
        }
    }
}

impl Volume {
    /// Copia dei soli metadati del volume (senza voci né stato).
    pub fn clone_meta(&self) -> Volume {
        Volume {
            index: self.index,
            offset: self.offset,
            size: self.size,
            fs: self.fs,
            label: self.label.clone(),
            partition_type: self.partition_type.clone(),
            cluster_size: self.cluster_size,
            error: self.error.clone(),
            entries: Vec::new(),
            state: None,
            foreign: None,
        }
    }
}

/// Scrive i dati di un file su `out`. Ritorna i byte scritti.
pub fn copy_entry(
    src: &dyn BlockSource,
    vol: &Volume,
    entry: &FileEntry,
    out: &mut dyn Write,
) -> io::Result<u64> {
    let mut buf = vec![0u8; 1024 * 1024];
    let mut written = 0u64;
    for ext in &entry.extents {
        match ext {
            Extent::Inline(data) => {
                out.write_all(data)?;
                written += data.len() as u64;
            }
            Extent::Sparse { length } => {
                buf.fill(0);
                let mut left = *length;
                while left > 0 {
                    let n = left.min(buf.len() as u64) as usize;
                    out.write_all(&buf[..n])?;
                    left -= n as u64;
                }
                written += length;
            }
            Extent::Disk { offset, length } => {
                let mut done = 0u64;
                while done < *length {
                    let n = (*length - done).min(buf.len() as u64) as usize;
                    read_exact_at(src, vol.offset + offset + done, &mut buf[..n])?;
                    out.write_all(&buf[..n])?;
                    done += n as u64;
                }
                written += length;
            }
        }
    }
    Ok(written)
}

/// Primi `max` byte di un file (per l'anteprima).
pub fn read_prefix(
    src: &dyn BlockSource,
    vol: &Volume,
    entry: &FileEntry,
    max: usize,
) -> io::Result<Vec<u8>> {
    struct Capped<'a> {
        out: &'a mut Vec<u8>,
        max: usize,
    }
    impl Write for Capped<'_> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let room = self.max - self.out.len();
            if room == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "pieno"));
            }
            let n = buf.len().min(room);
            self.out.extend_from_slice(&buf[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut out = Vec::with_capacity(max.min(entry.size as usize));
    let mut capped = Capped { out: &mut out, max };
    match copy_entry(src, vol, entry, &mut capped) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::WriteZero => {}
        Err(e) => return Err(e),
    }
    Ok(out)
}

/// Data e ora DOS (FAT, exFAT) in secondi Unix.
pub(crate) fn dos_datetime(date: u16, time: u16) -> Option<i64> {
    if date == 0 {
        return None;
    }
    let year = 1980 + (date >> 9) as i64;
    let month = ((date >> 5) & 0x0F) as i64;
    let day = (date & 0x1F) as i64;
    let hour = (time >> 11) as i64;
    let min = ((time >> 5) & 0x3F) as i64;
    let sec = ((time & 0x1F) * 2) as i64;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + min * 60 + sec)
}

/// Giorni dal 1970-01-01 (algoritmo di Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Decodifica UTF-16LE fermandosi al primo NUL o 0xFFFF (riempimento FAT).
pub(crate) fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|u| *u != 0 && *u != 0xFFFF)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Unisce cluster consecutivi in extent e tronca alla dimensione del file.
pub(crate) fn clusters_to_extents(
    clusters: &[u32],
    cluster_offset: impl Fn(u32) -> u64,
    cluster_size: u64,
    size: u64,
) -> Vec<Extent> {
    let mut extents: Vec<Extent> = Vec::new();
    let mut left = size;
    for &c in clusters {
        if left == 0 {
            break;
        }
        let len = left.min(cluster_size);
        let off = cluster_offset(c);
        if let Some(Extent::Disk { offset, length }) = extents.last_mut() {
            if *offset + *length == off {
                *length += len;
                left -= len;
                continue;
            }
        }
        extents.push(Extent::Disk {
            offset: off,
            length: len,
        });
        left -= len;
    }
    extents
}

/// Componente di percorso sicuro per il file system di destinazione.
pub fn sanitize_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim_end_matches(['.', ' ']).to_string();
    match trimmed.as_str() {
        "" | "." | ".." => "_".to_string(),
        _ => trimmed,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn found_folders_get_a_descriptive_name() {
        use super::*;
        let entry = |path: &str, is_dir: bool, ts: Option<i64>| FileEntry {
            id: 0,
            path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
            size: 1,
            is_dir,
            deleted: true,
            modified: ts,
            condition: Condition::Probable,
            integrity: Integrity::Unchecked,
            extents: Vec::new(),
        };
        let y2019 = 1_560_000_000; // giugno 2019
        let y2024 = 1_720_000_000; // luglio 2024
        let mut v = vec![
            entry("/Struttura precedente/Cartella ritrovata 1", true, None),
            entry(
                "/Struttura precedente/Cartella ritrovata 1/Foto",
                true,
                None,
            ),
            entry(
                "/Struttura precedente/Cartella ritrovata 1/Foto/a.JPG",
                false,
                Some(y2019),
            ),
            entry(
                "/Struttura precedente/Cartella ritrovata 1/Foto/b.jpg",
                false,
                Some(y2024),
            ),
            entry(
                "/Struttura precedente/Cartella ritrovata 1/c.mp4",
                false,
                None,
            ),
            entry("/Struttura precedente/Cartella ritrovata 12", true, None),
            entry("/Struttura precedente/Cartella ritrovata 13", true, None),
            entry(
                "/Struttura precedente/Cartella ritrovata 13/._x.pdf",
                false,
                None,
            ),
            entry("/Struttura precedente/Vacanze/x.pdf", false, None),
        ];
        describe_found_folders(&mut v);
        let top = "/Struttura precedente/Cartella ritrovata 1 (2 foto, 1 video, 2019-2024)";
        assert_eq!(v[0].path, top);
        assert_eq!(
            v[0].name,
            "Cartella ritrovata 1 (2 foto, 1 video, 2019-2024)"
        );
        assert_eq!(v[3].path, format!("{top}/Foto/b.jpg"));
        // Le cartelle senza file veri (vuote o con soli metadati del Mac) spariscono.
        assert_eq!(v.len(), 6);
        assert_eq!(v[5].path, "/Struttura precedente/Vacanze/x.pdf");
    }

    use super::*;

    #[test]
    fn dos_time() {
        // 2024-03-15 10:30:20
        let date = ((2024 - 1980) << 9) | (3 << 5) | 15;
        let time = (10 << 11) | (30 << 5) | 10;
        assert_eq!(dos_datetime(date, time), Some(1_710_498_620));
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_component(".."), "_");
        assert_eq!(sanitize_component("a:b?.txt"), "a_b_.txt");
        assert_eq!(sanitize_component("nome. "), "nome");
    }

    #[test]
    fn merge_clusters() {
        let ext = clusters_to_extents(&[2, 3, 4, 10], |c| c as u64 * 100, 100, 350);
        assert_eq!(
            ext,
            vec![
                Extent::Disk {
                    offset: 200,
                    length: 300
                },
                Extent::Disk {
                    offset: 1000,
                    length: 50
                },
            ]
        );
    }
}
