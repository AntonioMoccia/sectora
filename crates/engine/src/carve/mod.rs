//! Deep Scan: ricerca dei file per firma sui settori grezzi, senza usare
//! il file system. Funziona anche su dischi formattati o con file system
//! danneggiato, ma non recupera nomi e cartelle originali.

pub mod formats;
pub mod reader;

pub use formats::FileKind;

use crate::source::BlockSource;
use formats::Family;
use reader::ByteReader;
use serde::Serialize;
use std::collections::BTreeSet;

/// Un file trovato sulla sorgente.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Found {
    pub kind: FileKind,
    /// Offset in byte dall'inizio della sorgente.
    pub offset: u64,
    /// Lunghezza in byte.
    pub length: u64,
}

#[derive(Debug, Clone)]
pub struct CarveOptions {
    /// Passo di ricerca delle firme. 512 copre FAT, exFAT, NTFS, HFS+, APFS, ext4.
    pub step: u64,
    /// Tipi da cercare. Vuoto = tutti.
    pub kinds: BTreeSet<FileKind>,
    /// Dimensione dei blocchi letti in sequenza.
    pub chunk_size: usize,
    /// Intervallo da scansionare (default: tutta la sorgente).
    pub start: u64,
    pub end: Option<u64>,
    /// Cerca le firme dei file. `false` = sola lettura del disco (per la ricostruzione).
    pub detect: bool,
}

impl Default for CarveOptions {
    fn default() -> Self {
        Self {
            step: 512,
            kinds: BTreeSet::new(),
            chunk_size: 4 * 1024 * 1024,
            start: 0,
            end: None,
            detect: true,
        }
    }
}

/// Eventi emessi durante la scansione.
#[derive(Debug, Clone)]
pub enum Event {
    Progress {
        position: u64,
        total: u64,
    },
    Found(Found),
    /// Area illeggibile (settori danneggiati): viene saltata.
    ReadError {
        offset: u64,
        length: u64,
        message: String,
    },
}

/// Cosa fare dopo un evento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

/// Esegue il Deep Scan e ritorna i file trovati, in ordine di offset.
///
/// `on_event` riceve avanzamento, file trovati ed errori di lettura; se
/// ritorna [`Flow::Stop`] la scansione si interrompe e ritorna quanto
/// trovato fino a quel momento.
pub fn carve(
    src: &dyn BlockSource,
    opts: &CarveOptions,
    on_event: impl FnMut(&Event) -> Flow,
) -> Vec<Found> {
    carve_with(src, opts, on_event, &mut |_, _| {})
}

/// Come [`carve`], e in più passa ogni blocco letto a `observer(offset, dati)`:
/// così altre analisi (la ricostruzione del file system) usano la stessa
/// lettura del disco invece di rileggerlo.
pub fn carve_with(
    src: &dyn BlockSource,
    opts: &CarveOptions,
    mut on_event: impl FnMut(&Event) -> Flow,
    observer: &mut dyn FnMut(u64, &[u8]),
) -> Vec<Found> {
    let wanted = |k: FileKind| opts.kinds.is_empty() || opts.kinds.contains(&k);
    let families: Vec<Family> = {
        let mut f: Vec<Family> = FileKind::ALL
            .into_iter()
            .filter(|k| wanted(*k))
            .map(Family::of)
            .collect();
        f.dedup();
        f
    };

    let step = opts.step.max(1);
    let total = opts.end.unwrap_or(u64::MAX).min(src.size());
    let chunk_len = (opts.chunk_size as u64).max(step) / step * step;
    let mut chunk = vec![0u8; chunk_len as usize];
    let mut reader = ByteReader::new(src);
    let mut results = Vec::new();

    let mut pos = opts.start.div_ceil(step) * step;
    // Nessuna nuova firma prima di questo offset: siamo dentro un file già trovato.
    let mut skip_until = 0u64;

    while pos < total {
        let want = chunk_len.min(total - pos) as usize;
        let n = match read_chunk(src, pos, &mut chunk[..want]) {
            Ok(n) => n,
            Err(_) => {
                // Area danneggiata: rilettura settore per settore. I settori
                // illeggibili diventano zeri e il resto del blocco viene analizzato.
                let bad = reread_sectors(src, pos, &mut chunk[..want]);
                let mut stop = false;
                for (offset, length, message) in bad {
                    if on_event(&Event::ReadError {
                        offset,
                        length,
                        message,
                    }) == Flow::Stop
                    {
                        stop = true;
                    }
                }
                if stop {
                    break;
                }
                want
            }
        };
        if n == 0 {
            break;
        }
        observer(pos, &chunk[..n]);
        if !opts.detect {
            pos += n as u64;
            if on_event(&Event::Progress {
                position: pos,
                total,
            }) == Flow::Stop
            {
                break;
            }
            continue;
        }

        let mut rel = 0usize;
        while rel < n {
            let abs = pos + rel as u64;
            if abs >= skip_until {
                let head = &chunk[rel..n.min(rel + 16)];
                if let Some(family) = formats::detect(head) {
                    if families.contains(&family) {
                        if let Some((length, kind)) = formats::measure(&mut reader, family, abs) {
                            skip_until = abs + length;
                            if wanted(kind) {
                                let found = Found {
                                    kind,
                                    offset: abs,
                                    length,
                                };
                                let stop = on_event(&Event::Found(found.clone())) == Flow::Stop;
                                results.push(found);
                                if stop {
                                    return results;
                                }
                            }
                        }
                    }
                }
            }
            rel += step as usize;
        }

        pos += n as u64;
        if on_event(&Event::Progress {
            position: pos,
            total,
        }) == Flow::Stop
        {
            break;
        }
    }
    results
}

/// Rilegge `buf` settore per settore; ritorna le aree illeggibili (unite).
fn reread_sectors(src: &dyn BlockSource, offset: u64, buf: &mut [u8]) -> Vec<(u64, u64, String)> {
    let sector = src.sector_size().max(512) as usize;
    let mut bad: Vec<(u64, u64, String)> = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let n = sector.min(buf.len() - i);
        let piece = &mut buf[i..i + n];
        if let Err(e) = crate::source::read_exact_at(src, offset + i as u64, piece) {
            piece.fill(0);
            let at = offset + i as u64;
            match bad.last_mut() {
                Some(last) if last.0 + last.1 == at => last.1 += n as u64,
                _ => bad.push((at, n as u64, e.to_string())),
            }
        }
        i += n;
    }
    bad
}

fn read_chunk(src: &dyn BlockSource, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = src.read_at(offset + filled as u64, &mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}
