//! NTFS (dischi Windows, molti dischi esterni).
//!
//! Ogni file ha un record nella Master File Table (MFT). Alla cancellazione
//! il record viene solo marcato come libero: nome, cartella padre, date e
//! l'elenco esatto dei cluster (data run) restano finché il record non viene
//! riusato. Per questo su NTFS si recuperano bene anche i file frammentati.
//!
//! Limiti attuali: file compressi o cifrati NTFS, e file così frammentati da
//! avere un $ATTRIBUTE_LIST (record di estensione), vengono segnati come
//! [`Condition::Unsupported`].

use super::{Condition, Extent, FileEntry, FsType, Integrity, Volume};
use crate::source::{read_exact_at, BlockSource};
use std::collections::HashMap;

const ROOT: u64 = 5;
/// I primi record sono metadati di sistema ($MFT, $LogFile, $Bitmap, ...).
const FIRST_USER_RECORD: u64 = 24;

struct Geometry {
    record_size: u64,
    mft_offset: u64,
}

#[derive(Default, Clone)]
struct Record {
    in_use: bool,
    is_dir: bool,
    base_ref: u64,
    /// (parent, nome, namespace)
    names: Vec<(u64, String, u8)>,
    modified: Option<i64>,
    data: Option<DataAttr>,
    has_attr_list: bool,
    volume_name: Option<String>,
}

#[derive(Clone)]
enum DataAttr {
    Resident(Vec<u8>),
    NonResident {
        runs: Vec<(Option<u64>, u64)>, // (LCN o None se sparse, numero di cluster)
        size: u64,
        compressed_or_encrypted: bool,
    },
}

/// Applica le "fixup" (update sequence): gli ultimi 2 byte di ogni settore
/// del record sono salvati nell'array USA e vanno ripristinati.
fn apply_fixup(rec: &mut [u8]) -> bool {
    if rec.len() < 48 || &rec[0..4] != b"FILE" {
        return false;
    }
    let usa_off = u16::from_le_bytes([rec[4], rec[5]]) as usize;
    let usa_count = u16::from_le_bytes([rec[6], rec[7]]) as usize;
    if usa_count < 2 || usa_off + usa_count * 2 > rec.len() || (usa_count - 1) * 512 > rec.len() {
        return false;
    }
    let usn = [rec[usa_off], rec[usa_off + 1]];
    for i in 1..usa_count {
        let end = i * 512;
        if rec[end - 2..end] != usn {
            return false;
        }
        let fix = [rec[usa_off + i * 2], rec[usa_off + i * 2 + 1]];
        rec[end - 2..end].copy_from_slice(&fix);
    }
    true
}

fn filetime_to_unix(ft: u64) -> Option<i64> {
    if ft == 0 {
        return None;
    }
    Some((ft / 10_000_000) as i64 - 11_644_473_600)
}

/// Decodifica le data run: sequenza di (lunghezza, offset relativo del LCN).
fn decode_runs(mut p: &[u8]) -> Option<Vec<(Option<u64>, u64)>> {
    let mut runs = Vec::new();
    let mut lcn: i64 = 0;
    while let Some(&header) = p.first() {
        if header == 0 {
            break;
        }
        let len_size = (header & 0x0F) as usize;
        let off_size = (header >> 4) as usize;
        if len_size == 0 || len_size > 8 || off_size > 8 || p.len() < 1 + len_size + off_size {
            return None;
        }
        let mut len = 0u64;
        for i in 0..len_size {
            len |= (p[1 + i] as u64) << (8 * i);
        }
        if off_size == 0 {
            runs.push((None, len));
        } else {
            let mut off: i64 = 0;
            for i in 0..off_size {
                off |= (p[1 + len_size + i] as i64) << (8 * i);
            }
            // Estensione del segno.
            let shift = 64 - 8 * off_size as u32;
            off = (off << shift) >> shift;
            lcn += off;
            if lcn < 0 {
                return None;
            }
            runs.push((Some(lcn as u64), len));
        }
        p = &p[1 + len_size + off_size..];
    }
    Some(runs)
}

fn parse_record(rec: &[u8]) -> Option<Record> {
    let flags = u16::from_le_bytes([rec[22], rec[23]]);
    let mut r = Record {
        in_use: flags & 0x01 != 0,
        is_dir: flags & 0x02 != 0,
        base_ref: u64::from_le_bytes(rec[32..40].try_into().ok()?) & 0x0000_FFFF_FFFF_FFFF,
        ..Default::default()
    };
    let used = (u32::from_le_bytes(rec[24..28].try_into().ok()?) as usize).min(rec.len());
    let mut pos = u16::from_le_bytes([rec[20], rec[21]]) as usize;

    while pos + 16 <= used {
        let typ = u32::from_le_bytes(rec[pos..pos + 4].try_into().ok()?);
        if typ == 0xFFFF_FFFF {
            break;
        }
        let len = u32::from_le_bytes(rec[pos + 4..pos + 8].try_into().ok()?) as usize;
        if len < 16 || pos + len > used {
            break;
        }
        let a = &rec[pos..pos + len];
        let non_resident = a[8] != 0;
        // Header minimo: 24 byte se residente, 64 se non residente.
        if len < if non_resident { 64 } else { 24 } {
            pos += len;
            continue;
        }
        let name_len = a[9];
        let attr_flags = u16::from_le_bytes([a[12], a[13]]);

        let resident_value = || -> Option<&[u8]> {
            let vlen = u32::from_le_bytes(a[16..20].try_into().ok()?) as usize;
            let voff = u16::from_le_bytes([a[20], a[21]]) as usize;
            a.get(voff..voff + vlen)
        };

        match typ {
            0x20 => r.has_attr_list = true,
            0x30 if !non_resident => {
                if let Some(v) = resident_value() {
                    if v.len() >= 66 {
                        let parent =
                            u64::from_le_bytes(v[0..8].try_into().ok()?) & 0x0000_FFFF_FFFF_FFFF;
                        let nlen = v[64] as usize;
                        let ns = v[65];
                        if let Some(raw) = v.get(66..66 + nlen * 2) {
                            let units: Vec<u16> = raw
                                .chunks_exact(2)
                                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                                .collect();
                            r.names.push((parent, String::from_utf16_lossy(&units), ns));
                        }
                        if r.modified.is_none() {
                            r.modified =
                                filetime_to_unix(u64::from_le_bytes(v[16..24].try_into().ok()?));
                        }
                    }
                }
            }
            0x10 if !non_resident => {
                // $STANDARD_INFORMATION: la data di modifica più affidabile.
                if let Some(v) = resident_value() {
                    if v.len() >= 16 {
                        r.modified =
                            filetime_to_unix(u64::from_le_bytes(v[8..16].try_into().ok()?));
                    }
                }
            }
            0x60 if !non_resident => {
                if let Some(v) = resident_value() {
                    r.volume_name = Some(super::utf16le(v));
                }
            }
            0x80 if name_len == 0 && r.data.is_none() => {
                if non_resident {
                    let start_vcn = u64::from_le_bytes(a[16..24].try_into().ok()?);
                    let runs_off = u16::from_le_bytes([a[32], a[33]]) as usize;
                    let size = u64::from_le_bytes(a[48..56].try_into().ok()?);
                    if start_vcn == 0 {
                        if let Some(runs) = a.get(runs_off..).and_then(decode_runs) {
                            r.data = Some(DataAttr::NonResident {
                                runs,
                                size,
                                compressed_or_encrypted: attr_flags & 0x4001 != 0,
                            });
                        }
                    }
                } else if let Some(v) = resident_value() {
                    r.data = Some(DataAttr::Resident(v.to_vec()));
                }
            }
            _ => {}
        }
        pos += len;
    }
    Some(r)
}

fn runs_to_extents(runs: &[(Option<u64>, u64)], cluster_size: u64, size: u64) -> Vec<Extent> {
    let mut out = Vec::new();
    let mut left = size;
    for (lcn, count) in runs {
        if left == 0 {
            break;
        }
        let len = (count * cluster_size).min(left);
        out.push(match lcn {
            Some(l) => Extent::Disk {
                offset: l * cluster_size,
                length: len,
            },
            None => Extent::Sparse { length: len },
        });
        left -= len;
    }
    out
}

pub fn scan(src: &dyn BlockSource, vol: &mut Volume) -> Result<(), String> {
    let mut b = vec![0u8; 512];
    read_exact_at(src, 0, &mut b).map_err(|e| e.to_string())?;
    let bps = u16::from_le_bytes([b[11], b[12]]) as u64;
    let spc_raw = b[13];
    let spc = if spc_raw > 0x80 {
        1u64 << (256 - spc_raw as u32)
    } else {
        spc_raw as u64
    };
    if !matches!(bps, 512 | 1024 | 2048 | 4096) || spc == 0 {
        return Err("settore di avvio NTFS non valido".into());
    }
    let cluster_size = bps * spc;
    let mft_lcn = u64::from_le_bytes(b[48..56].try_into().unwrap());
    let cpr = b[64] as i8;
    let record_size = if cpr < 0 {
        1u64 << (-cpr as u32)
    } else {
        cpr as u64 * cluster_size
    };
    if !(256..=65536).contains(&record_size) {
        return Err("dimensione dei record MFT non valida".into());
    }
    let g = Geometry {
        record_size,
        mft_offset: mft_lcn * cluster_size,
    };
    vol.fs = Some(FsType::Ntfs);
    vol.cluster_size = cluster_size as u32;

    // Record 0 = $MFT: le sue data run dicono dove sta tutta la MFT.
    let mut rec0 = vec![0u8; g.record_size as usize];
    read_exact_at(src, g.mft_offset, &mut rec0).map_err(|e| format!("MFT illeggibile: {e}"))?;
    if !apply_fixup(&mut rec0) {
        return Err("record $MFT danneggiato".into());
    }
    let mft = parse_record(&rec0).ok_or("record $MFT non valido")?;
    let mft_extents = match mft.data {
        Some(DataAttr::NonResident { runs, size, .. }) => {
            runs_to_extents(&runs, cluster_size, size)
        }
        _ => return Err("$MFT senza dati".into()),
    };

    // Lettura di tutti i record.
    let mut records: HashMap<u64, Record> = HashMap::new();
    let mut number = 0u64;
    let mut buf = vec![0u8; (g.record_size * 256) as usize];
    for ext in &mft_extents {
        let Extent::Disk { offset, length } = ext else {
            number += ext.len() / g.record_size;
            continue;
        };
        let mut done = 0;
        while done < *length {
            let n = (*length - done).min(buf.len() as u64) as usize;
            if read_exact_at(src, offset + done, &mut buf[..n]).is_err() {
                number += n as u64 / g.record_size;
                done += n as u64;
                continue;
            }
            for chunk in buf[..n].chunks_exact_mut(g.record_size as usize) {
                if apply_fixup(chunk) {
                    if let Some(r) = parse_record(chunk) {
                        records.insert(number, r);
                    }
                }
                number += 1;
            }
            done += n as u64;
        }
    }

    vol.label = records
        .get(&3)
        .and_then(|r| r.volume_name.clone())
        .filter(|s| !s.is_empty());

    // $Bitmap (record 6): cluster oggi allocati, per capire se un file cancellato è stato sovrascritto.
    let bitmap: Option<Vec<u8>> = records.get(&6).and_then(|r| match &r.data {
        Some(DataAttr::NonResident { runs, size, .. }) if *size <= 256 << 20 => {
            let vol_tmp = Volume {
                offset: 0,
                ..vol.clone()
            };
            let entry = FileEntry {
                id: 6,
                path: String::new(),
                name: String::new(),
                size: *size,
                is_dir: false,
                deleted: false,
                modified: None,
                condition: Condition::Intact,
                integrity: Integrity::Unchecked,
                extents: runs_to_extents(runs, cluster_size, *size),
            };
            let mut out = Vec::new();
            super::copy_entry(src, &vol_tmp, &entry, &mut out).ok()?;
            Some(out)
        }
        _ => None,
    });
    let allocated = |lcn: u64| is_allocated(bitmap.as_deref(), lcn);
    let entries = build_entries(&records, cluster_size, &allocated, None);

    // Per la ricostruzione: i record "FILE" fuori dalla MFT attuale appartengono
    // al file system precedente (formattazione rapida).
    let live: Vec<(u64, u64)> = mft_extents
        .iter()
        .filter_map(|e| match e {
            Extent::Disk { offset, length } => Some((*offset, offset + length)),
            _ => None,
        })
        .collect();
    vol.state = Some(super::FsState::Ntfs(State {
        record_size: g.record_size,
        cluster_size,
        live,
        bitmap,
        old: HashMap::new(),
        mft0: None,
        foreign: false,
    }));
    vol.entries = entries;
    Ok(())
}

/// Voci del volume a partire dai record MFT. Con `rebuilt` i record vengono dal
/// file system precedente: tutti cancellati, sotto `REBUILT_DIR`, id distinti.
/// `base`: posizione del vecchio volume NTFS rispetto a quello attuale (byte),
/// per i record di un NTFS precedente su un volume oggi FAT/exFAT.
fn build_entries(
    records: &HashMap<u64, Record>,
    cluster_size: u64,
    allocated: &dyn Fn(u64) -> bool,
    rebuilt: Option<(&str, u64)>,
) -> Vec<FileEntry> {
    // Nome preferito: Win32 (1) o Win32&DOS (3), poi POSIX (0), infine DOS 8.3 (2).
    let best_name = |r: &Record| -> Option<(u64, String)> {
        r.names
            .iter()
            .min_by_key(|(_, _, ns)| match ns {
                1 | 3 => 0,
                0 => 1,
                _ => 2,
            })
            .map(|(p, n, _)| (*p, n.clone()))
    };

    let mut path_cache: HashMap<u64, (String, bool)> = HashMap::new();
    let mut entries = Vec::new();
    let mut ids: Vec<u64> = records
        .keys()
        .copied()
        .filter(|n| *n >= FIRST_USER_RECORD)
        .collect();
    ids.sort_unstable();

    for id in ids {
        let r = &records[&id];
        if r.base_ref != 0 {
            continue; // record di estensione
        }
        let Some((parent, name)) = best_name(r) else {
            continue;
        };
        if parent == ROOT && name.starts_with('$') {
            continue;
        }
        let (parent_path, parent_deleted) =
            resolve_path(parent, records, &best_name, &mut path_cache, 0);
        if parent_path.starts_with("/$Extend") {
            continue; // metadati di sistema NTFS
        }
        let deleted = rebuilt.is_some() || !r.in_use || parent_deleted;

        let (size, extents, condition) = match (&r.data, r.is_dir) {
            (_, true) => (
                0,
                Vec::new(),
                if deleted {
                    Condition::Recoverable
                } else {
                    Condition::Intact
                },
            ),
            (None, false) if r.has_attr_list => (0, Vec::new(), Condition::Unsupported),
            (None, false) => (
                0,
                Vec::new(),
                if deleted {
                    Condition::Recoverable
                } else {
                    Condition::Intact
                },
            ),
            (Some(DataAttr::Resident(v)), false) => (
                v.len() as u64,
                vec![Extent::Inline(v.clone())],
                if deleted {
                    Condition::Recoverable
                } else {
                    Condition::Intact
                },
            ),
            (
                Some(DataAttr::NonResident {
                    runs,
                    size,
                    compressed_or_encrypted,
                }),
                false,
            ) => {
                let covered: u64 = runs.iter().map(|(_, c)| c * cluster_size).sum();
                if *compressed_or_encrypted || (covered < *size && r.has_attr_list) {
                    (*size, Vec::new(), Condition::Unsupported)
                } else {
                    let mut ext = runs_to_extents(runs, cluster_size, *size);
                    if let Some((_, base)) = rebuilt {
                        for e in ext.iter_mut() {
                            if let Extent::Disk { offset, .. } = e {
                                *offset += base;
                            }
                        }
                    }
                    let cond = if !deleted {
                        Condition::Intact
                    } else if runs
                        .iter()
                        .filter_map(|(l, c)| l.map(|l| (l, *c)))
                        .any(|(l, c)| (l..l + c).any(allocated))
                    {
                        Condition::Overwritten
                    } else {
                        Condition::Recoverable
                    };
                    (*size, ext, cond)
                }
            }
        };

        let prefix = rebuilt.map_or("", |(p, _)| p);
        entries.push(FileEntry {
            id: if rebuilt.is_some() { id | 1 << 40 } else { id },
            path: format!("{prefix}{parent_path}/{name}"),
            name,
            size,
            is_dir: r.is_dir,
            deleted,
            modified: r.modified,
            condition,
            integrity: Integrity::Unchecked,
            extents,
        });
    }
    entries
}

fn is_allocated(bitmap: Option<&[u8]>, lcn: u64) -> bool {
    bitmap.is_some_and(|bm| {
        bm.get((lcn / 8) as usize)
            .is_some_and(|b| b & (1 << (lcn % 8)) != 0)
    })
}

/// Stato per la ricostruzione: durante la lettura completa del volume si
/// raccolgono i record "FILE" fuori dalla MFT attuale. Dopo una formattazione
/// rapida la nuova MFT ne sovrascrive solo i primi, e il resto della vecchia
/// resta sul disco. Il numero del record è scritto nel record stesso (offset 44).
#[derive(Clone)]
pub(crate) struct State {
    record_size: u64,
    cluster_size: u64,
    live: Vec<(u64, u64)>,
    bitmap: Option<Vec<u8>>,
    old: HashMap<u64, Record>,
    /// Dove è stato trovato il record di $MFT (numero 0) e il primo cluster
    /// che dichiara per la MFT: ne derivano dimensione dei cluster e inizio del
    /// vecchio volume.
    mft0: Option<(u64, u64)>,
    /// Record di un NTFS precedente su un volume che oggi non è NTFS.
    foreign: bool,
}

impl State {
    /// Raccolta dei record di un NTFS precedente su un volume FAT o exFAT.
    pub(crate) fn foreign() -> State {
        State {
            record_size: 1024,
            cluster_size: 4096,
            live: Vec::new(),
            bitmap: None,
            old: HashMap::new(),
            mft0: None,
            foreign: true,
        }
    }

    /// Cluster del vecchio NTFS e suo inizio rispetto al volume attuale: il
    /// record di $MFT si trova a `base + lcn * cluster`. Si preferisce la
    /// soluzione con il vecchio volume allineato a quello attuale (stessa
    /// partizione), poi un inizio allineato a 1 MiB, infine 4 KiB / stesso inizio.
    fn foreign_geometry(&self) -> (u64, u64) {
        let Some((at, lcn)) = self.mft0 else {
            return (4096, 0);
        };
        let sizes = (9..=21).map(|s| 1u64 << s);
        let base = |cs: u64| at.checked_sub(lcn.checked_mul(cs)?);
        sizes
            .clone()
            .find(|&cs| base(cs) == Some(0))
            .or_else(|| {
                sizes
                    .clone()
                    .find(|&cs| base(cs).is_some_and(|b| b % (1 << 20) == 0))
            })
            .map(|cs| (cs, base(cs).unwrap_or(0)))
            .unwrap_or((4096, 0))
    }
    pub(crate) fn feed(&mut self, view: &dyn BlockSource, rel: u64, chunk: &[u8]) {
        let rs = self.record_size as usize;
        let mut i = ((512 - rel % 512) % 512) as usize;
        let mut tmp = vec![0u8; rs];
        while i + 4 <= chunk.len() {
            let abs = rel + i as u64;
            if &chunk[i..i + 4] == b"FILE" && !self.live.iter().any(|(a, b)| abs >= *a && abs < *b)
            {
                let ok = if i + rs <= chunk.len() {
                    tmp.copy_from_slice(&chunk[i..i + rs]);
                    true
                } else {
                    read_exact_at(view, abs, &mut tmp).is_ok()
                };
                let usa_off = u16::from_le_bytes([tmp[4], tmp[5]]);
                if ok && usa_off >= 48 && apply_fixup(&mut tmp) {
                    let number = u32::from_le_bytes(tmp[44..48].try_into().unwrap()) as u64;
                    if let Some(r) = parse_record(&tmp) {
                        if number == 0 && self.mft0.is_none() {
                            if let Some(DataAttr::NonResident { runs, .. }) = &r.data {
                                if let Some((Some(lcn), _)) = runs.first() {
                                    self.mft0 = Some((abs, *lcn));
                                }
                            }
                        }
                        self.old.entry(number).or_insert(r);
                    }
                }
            }
            i += 512;
        }
    }

    /// Voci provvisorie per l'anteprima in tempo reale: l'albero dai record
    /// raccolti finora (i record contengono già nome e cartella madre).
    pub(crate) fn live_entries(&self) -> Vec<FileEntry> {
        if self.foreign {
            if !self.old.contains_key(&ROOT) && self.old.len() < 8 {
                return Vec::new();
            }
            let (cs, base) = self.foreign_geometry();
            return build_entries(
                &self.old,
                cs,
                &|_| false,
                Some((super::REBUILT_NTFS_DIR, base)),
            );
        }
        let allocated = |lcn: u64| is_allocated(self.bitmap.as_deref(), lcn);
        build_entries(
            &self.old,
            self.cluster_size,
            &allocated,
            Some((super::REBUILT_DIR, 0)),
        )
    }

    pub(crate) fn finish(self, _view: &dyn BlockSource) -> Vec<FileEntry> {
        if self.foreign {
            // Un volume FAT/exFAT contiene anche record "FILE" per caso solo raramente:
            // senza la radice o con pochissimi record non è un vecchio NTFS.
            if !self.old.contains_key(&ROOT) && self.old.len() < 8 {
                return Vec::new();
            }
            let (cs, base) = self.foreign_geometry();
            return build_entries(
                &self.old,
                cs,
                &|_| false,
                Some((super::REBUILT_NTFS_DIR, base)),
            );
        }
        let bitmap = self.bitmap;
        let allocated = |lcn: u64| is_allocated(bitmap.as_deref(), lcn);
        build_entries(
            &self.old,
            self.cluster_size,
            &allocated,
            Some((super::REBUILT_DIR, 0)),
        )
    }
}

/// Percorso della directory `id`. Se la catena dei padri è interrotta
/// (record riusato o mancante) il file finisce in `/$Orfani`.
fn resolve_path(
    id: u64,
    records: &HashMap<u64, Record>,
    best_name: &dyn Fn(&Record) -> Option<(u64, String)>,
    cache: &mut HashMap<u64, (String, bool)>,
    depth: u32,
) -> (String, bool) {
    if id == ROOT {
        return (String::new(), false);
    }
    if let Some(v) = cache.get(&id) {
        return v.clone();
    }
    let result = match records.get(&id) {
        Some(r) if r.is_dir && depth < 256 => match best_name(r) {
            Some((parent, name)) if parent != id => {
                let (pp, pdel) = resolve_path(parent, records, best_name, cache, depth + 1);
                (format!("{pp}/{name}"), pdel || !r.in_use)
            }
            _ => ("/$Orfani".to_string(), true),
        },
        _ => ("/$Orfani".to_string(), true),
    };
    cache.insert(id, result.clone());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_runs_with_negative_offset_and_sparse() {
        // 0x21: 1 byte lunghezza, 2 byte offset. Run 1: 16 cluster a LCN 0x1000.
        // Run 2: 8 cluster a offset -0x800 (LCN 0x800). Run 3: sparse 4 cluster.
        let raw = [
            0x21, 0x10, 0x00, 0x10, 0x21, 0x08, 0x00, 0xF8, 0x01, 0x04, 0x00,
        ];
        let runs = decode_runs(&raw).unwrap();
        assert_eq!(runs, vec![(Some(0x1000), 16), (Some(0x800), 8), (None, 4)]);
    }

    #[test]
    fn filetime() {
        // 2024-01-01T00:00:00Z
        assert_eq!(
            filetime_to_unix(133_485_408_000_000_000),
            Some(1_704_067_200)
        );
    }
}
