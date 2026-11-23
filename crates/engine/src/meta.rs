//! Esportazione delle sole strutture del file system, per la diagnosi.
//!
//! Salva i settori di avvio e le FAT di ogni volume, più ogni settore del disco
//! che contiene voci di directory (FAT, exFAT) o record MFT (NTFS): nomi,
//! dimensioni e posizioni dei file, **mai il loro contenuto**. Con il file si
//! ricostruisce un'immagine "vuota" del disco su cui riprodurre la scansione
//! (`scripts/meta-to-img.py`).
//!
//! Formato: `SECTMETA1`, dimensione del disco (u64 LE), poi record
//! `offset (u64 LE) | lunghezza (u32 LE) | dati`.

use crate::fs::{self, FsType};
use crate::source::{read_exact_at, BlockSource};
use std::io::{self, Write};

const SECTOR: usize = 512;
const CHUNK: usize = 4 << 20;

/// Esito dell'esportazione.
#[derive(Debug, Default, Clone, Copy)]
pub struct MetaReport {
    /// Blocchi contigui salvati.
    pub records: u64,
    /// Byte di strutture salvati.
    pub bytes: u64,
    /// Byte illeggibili saltati.
    pub unreadable: u64,
}

/// Il settore contiene strutture di directory o record MFT.
fn is_metadata(s: &[u8]) -> bool {
    if s.iter().all(|&b| b == 0) {
        return false;
    }
    let dot_dir = &s[0..11] == b".          " && &s[32..34] == b"..";
    dot_dir
        || fs::fat::plausible_entries(s)
        || fs::exfat::starts_with_file_set(s)
        || fs::exfat::continues_file_sets(s)
        || s.starts_with(b"FILE0")
        || s.starts_with(b"FILE*")
        || is_boot_sector(s)
}

/// Settore di avvio di un file system (anche copie di riserva o di file system
/// precedenti, es. quella di NTFS nell'ultimo settore del volume).
fn is_boot_sector(s: &[u8]) -> bool {
    s[510..512] == [0x55, 0xAA]
        && (&s[3..11] == b"NTFS    " || &s[3..11] == b"EXFAT   " || fs::detect(s).is_some())
}

/// Settori da salvare dopo uno che inizia un record MFT: un record occupa 1 KiB
/// (2 settori) e la verifica delle "fixup" richiede anche il secondo.
const RECORD_TAIL: u32 = 1;

/// Aree di sole strutture (inizio del disco, avvio e FAT di ogni volume).
fn metadata_regions(src: &dyn BlockSource) -> Vec<(u64, u64)> {
    let mut regions = vec![(0, (1u64 << 20).min(src.size()))];
    for v in fs::scan_volumes(src) {
        let mut boot = vec![0u8; SECTOR];
        if read_exact_at(src, v.offset, &mut boot).is_err() {
            continue;
        }
        let len = match v.fs {
            Some(FsType::Fat12 | FsType::Fat16 | FsType::Fat32) => fs::fat::metadata_len(&boot),
            Some(FsType::ExFat) => {
                let shift = boot[108] as u32;
                (9..=12).contains(&shift).then(|| {
                    u32::from_le_bytes(boot[88..92].try_into().unwrap()) as u64 * (1 << shift)
                })
            }
            _ => Some(1 << 20),
        };
        if let Some(len) = len {
            regions.push((v.offset, len.min(v.size).min(256 << 20)));
        }
    }
    regions
}

/// Scrive su `out` le strutture del file system di `src` (vedi il modulo).
pub fn dump_metadata(
    src: &dyn BlockSource,
    out: &mut dyn Write,
    mut on_progress: impl FnMut(u64, u64),
) -> io::Result<MetaReport> {
    let total = src.size();
    let regions = metadata_regions(src);
    let in_region = |pos: u64| regions.iter().any(|&(o, l)| pos >= o && pos < o + l);

    out.write_all(b"SECTMETA1")?;
    out.write_all(&total.to_le_bytes())?;
    let mut report = MetaReport::default();
    let mut run: Vec<u8> = Vec::new();
    let mut run_start = 0u64;
    let flush = |run: &mut Vec<u8>,
                 start: u64,
                 report: &mut MetaReport,
                 out: &mut dyn Write|
     -> io::Result<()> {
        if run.is_empty() {
            return Ok(());
        }
        out.write_all(&start.to_le_bytes())?;
        out.write_all(&(run.len() as u32).to_le_bytes())?;
        out.write_all(run)?;
        report.records += 1;
        report.bytes += run.len() as u64;
        run.clear();
        Ok(())
    };

    let mut buf = vec![0u8; CHUNK];
    let mut pos = 0u64;
    let mut tail = 0u32;
    while pos < total {
        let want = CHUNK.min((total - pos) as usize);
        if read_exact_at(src, pos, &mut buf[..want]).is_err() {
            flush(&mut run, run_start, &mut report, out)?;
            report.unreadable += want as u64;
            pos += want as u64;
            continue;
        }
        for (i, s) in buf[..want].chunks(SECTOR).enumerate() {
            let at = pos + (i * SECTOR) as u64;
            let mut keep = s.len() == SECTOR && (in_region(at) || is_metadata(s));
            if tail > 0 {
                keep = true;
                tail -= 1;
            }
            if s.starts_with(b"FILE") {
                tail = RECORD_TAIL;
            }
            if keep {
                if run.is_empty() {
                    run_start = at;
                }
                run.extend_from_slice(s);
                if run.len() >= 16 << 20 {
                    flush(&mut run, run_start, &mut report, out)?;
                }
            } else {
                flush(&mut run, run_start, &mut report, out)?;
            }
        }
        pos += want as u64;
        on_progress(pos, total);
    }
    flush(&mut run, run_start, &mut report, out)?;
    out.flush()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemorySource;

    #[test]
    fn keeps_directories_and_drops_file_contents() {
        let mut disk = vec![0u8; 8 << 20];
        // Contenuto di un file (testo) e un settore di directory FAT con "." e "..".
        disk[(3 << 20)..(3 << 20) + 13].copy_from_slice(b"dati privati!");
        let dir = (5 << 20) as usize;
        disk[dir..dir + 11].copy_from_slice(b".          ");
        disk[dir + 11] = 0x10;
        disk[dir + 32..dir + 43].copy_from_slice(b"..         ");
        disk[dir + 43] = 0x10;
        let src = MemorySource::new(disk, "prova");
        let mut out = Vec::new();
        let r = dump_metadata(&src, &mut out, |_, _| {}).unwrap();
        assert_eq!(&out[..9], b"SECTMETA1");
        assert!(!out.windows(13).any(|w| w == b"dati privati!"));
        assert!(out.windows(11).any(|w| w == b".          "));
        assert!(r.records >= 1);
    }

    #[test]
    fn keeps_whole_mft_records() {
        let mut disk = vec![0u8; 8 << 20];
        let rec = 6 << 20;
        disk[rec..rec + 5].copy_from_slice(b"FILE0");
        disk[rec + 1022..rec + 1024].copy_from_slice(b"\x07\x00");
        let mut out = Vec::new();
        dump_metadata(&MemorySource::new(disk, "prova"), &mut out, |_, _| {}).unwrap();
        // Record: offset (8) + lunghezza (4) + 1024 byte.
        let pos = out.windows(5).position(|w| w == b"FILE0").unwrap();
        let len = u32::from_le_bytes(out[pos - 4..pos].try_into().unwrap());
        assert_eq!(len, 1024);
    }
}
