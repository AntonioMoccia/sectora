//! FAT12, FAT16, FAT32.
//!
//! Quando un file viene cancellato, FAT marca il primo byte del nome con
//! 0xE5 e azzera la sua catena di cluster. Restano: nome (meno il primo
//! carattere), dimensione, data e primo cluster. Si assume quindi che i
//! dati siano contigui a partire dal primo cluster ([`Condition::Probable`]).
//! Il primo carattere del nome viene ricostruito dal checksum del nome lungo.

use super::{
    clusters_to_extents, dos_datetime, utf16le, Condition, FileEntry, FsType, Integrity, Volume,
};
use crate::carve::reader::ByteReader;
use crate::source::BlockSource;
use std::collections::{BTreeMap, HashSet};

pub fn detect(b: &[u8]) -> Option<FsType> {
    let bpb = Bpb::parse(b)?;
    Some(bpb.fs)
}

#[derive(Clone)]
struct Bpb {
    fs: FsType,
    cluster_size: u64,
    fat_offset: u64,
    root_dir_offset: u64,
    root_entries: u64,
    data_offset: u64,
    cluster_count: u32,
    root_cluster: u32,
    label: Option<String>,
}

impl Bpb {
    fn parse(b: &[u8]) -> Option<Bpb> {
        if b.len() < 512 || b[510..512] != [0x55, 0xAA] || !matches!(b[0], 0xEB | 0xE9) {
            return None;
        }
        let le16 = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as u64;
        let le32 = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as u64;
        let bps = le16(11);
        let spc = b[13] as u64;
        let reserved = le16(14);
        let fats = b[16] as u64;
        let root_entries = le16(17);
        let total = if le16(19) != 0 { le16(19) } else { le32(32) };
        let fat_size = if le16(22) != 0 { le16(22) } else { le32(36) };
        if !matches!(bps, 512 | 1024 | 2048 | 4096)
            || spc == 0
            || !spc.is_power_of_two()
            || reserved == 0
            || !(1..=2).contains(&fats)
            || fat_size == 0
            || total == 0
        {
            return None;
        }
        let root_dir_sectors = (root_entries * 32).div_ceil(bps);
        let first_data = reserved + fats * fat_size + root_dir_sectors;
        if first_data >= total {
            return None;
        }
        let clusters = (total - first_data) / spc;
        // FAT32 ha sempre a zero il campo "settori per FAT" a 16 bit e la radice
        // fissa: così lo riconosce anche Linux, pure con pochi cluster (volumi piccoli
        // formattati FAT32 per forza). Altrimenti decide il numero di cluster.
        let fs = if le16(22) == 0 && root_entries == 0 {
            FsType::Fat32
        } else if clusters < 4085 {
            FsType::Fat12
        } else {
            FsType::Fat16
        };
        let label_at = if fs == FsType::Fat32 { 71 } else { 43 };
        let label = String::from_utf8_lossy(&b[label_at..label_at + 11])
            .trim()
            .to_string();
        Some(Bpb {
            fs,
            cluster_size: bps * spc,
            fat_offset: reserved * bps,
            root_dir_offset: (reserved + fats * fat_size) * bps,
            root_entries,
            data_offset: first_data * bps,
            cluster_count: clusters as u32,
            root_cluster: if fs == FsType::Fat32 {
                le32(44) as u32
            } else {
                0
            },
            label: (!label.is_empty() && label != "NO NAME").then_some(label),
        })
    }
}

struct Fat<'a> {
    bpb: Bpb,
    r: ByteReader<'a>,
    next_id: u64,
    entries: Vec<FileEntry>,
    visited: HashSet<u32>,
}

impl Fat<'_> {
    fn cluster_offset(&self, c: u32) -> u64 {
        self.bpb.data_offset + (c as u64 - 2) * self.bpb.cluster_size
    }

    fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && c < self.bpb.cluster_count + 2
    }

    /// Valore della FAT per il cluster `c` (0 = libero).
    fn fat_entry(&mut self, c: u32) -> Option<u32> {
        let base = self.bpb.fat_offset;
        match self.bpb.fs {
            FsType::Fat12 => {
                let off = c as u64 + c as u64 / 2;
                let v = self.r.u16_le(base + off)?;
                Some(if c & 1 == 1 {
                    (v >> 4) as u32
                } else {
                    (v & 0x0FFF) as u32
                })
            }
            FsType::Fat16 => self.r.u16_le(base + c as u64 * 2).map(|v| v as u32),
            _ => self.r.u32_le(base + c as u64 * 4).map(|v| v & 0x0FFF_FFFF),
        }
    }

    fn is_end(&self, v: u32) -> bool {
        match self.bpb.fs {
            FsType::Fat12 => v >= 0xFF8,
            FsType::Fat16 => v >= 0xFFF8,
            _ => v >= 0x0FFF_FFF8,
        }
    }

    fn chain(&mut self, first: u32, max: usize) -> Vec<u32> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut c = first;
        while self.valid_cluster(c) && out.len() < max && seen.insert(c) {
            out.push(c);
            match self.fat_entry(c) {
                Some(v) if !self.is_end(v) && v >= 2 => c = v,
                _ => break,
            }
        }
        out
    }

    fn read_clusters(&mut self, clusters: &[u32]) -> Vec<u8> {
        let cs = self.bpb.cluster_size as usize;
        let mut data = vec![0u8; clusters.len() * cs];
        for (i, &c) in clusters.iter().enumerate() {
            let off = self.cluster_offset(c);
            if self.r.read(off, &mut data[i * cs..(i + 1) * cs]).is_none() {
                data.truncate(i * cs);
                break;
            }
        }
        data
    }

    fn root_data(&mut self) -> Vec<u8> {
        if self.bpb.fs == FsType::Fat32 {
            let chain = self.chain(self.bpb.root_cluster, 65536);
            self.read_clusters(&chain)
        } else {
            let mut data = vec![0u8; (self.bpb.root_entries * 32) as usize];
            if self.r.read(self.bpb.root_dir_offset, &mut data).is_none() {
                data.clear();
            }
            data
        }
    }

    fn walk(&mut self, data: &[u8], parent: &str, parent_deleted: bool, depth: u32) {
        if depth > 64 {
            return;
        }
        let mut lfn: Vec<(u8, [u16; 13])> = Vec::new();
        let mut subdirs: Vec<(String, u32, bool)> = Vec::new();

        for e in data.chunks_exact(32) {
            let first = e[0];
            if first == 0x00 {
                break;
            }
            let attr = e[11];
            if attr == 0x0F {
                let mut units = [0u16; 13];
                for (i, off) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
                    .iter()
                    .enumerate()
                {
                    units[i] = u16::from_le_bytes([e[*off], e[off + 1]]);
                }
                lfn.push((e[13], units));
                continue;
            }
            let pending_lfn = std::mem::take(&mut lfn);
            if attr & 0x08 != 0 {
                continue; // etichetta del volume
            }
            if &e[0..2] == b". " || &e[0..2] == b".." {
                continue;
            }
            let deleted = first == 0xE5 || parent_deleted;
            let mut short = [0u8; 11];
            short.copy_from_slice(&e[0..11]);
            if short[0] == 0x05 {
                short[0] = 0xE5;
            }

            let name = match long_name(&pending_lfn, &mut short, first == 0xE5) {
                Some(n) => n,
                None => short_name(&short, first == 0xE5, e[12]),
            };
            if name.is_empty() {
                continue;
            }

            let le16 = |o: usize| u16::from_le_bytes([e[o], e[o + 1]]);
            let cluster_hi = if self.bpb.fs == FsType::Fat32 {
                le16(20) as u32
            } else {
                0
            };
            let first_cluster = (cluster_hi << 16) | le16(26) as u32;
            let size = u32::from_le_bytes([e[28], e[29], e[30], e[31]]) as u64;
            let is_dir = attr & 0x10 != 0;
            let path = format!("{parent}/{name}");
            let modified = dos_datetime(le16(24), le16(22));

            let (extents, condition) = if is_dir || size == 0 {
                (
                    Vec::new(),
                    if deleted {
                        Condition::Probable
                    } else {
                        Condition::Intact
                    },
                )
            } else if !self.valid_cluster(first_cluster) {
                (Vec::new(), Condition::Unsupported)
            } else if !deleted {
                let max = size.div_ceil(self.bpb.cluster_size) as usize;
                let chain = self.chain(first_cluster, max);
                let ext = clusters_to_extents(
                    &chain,
                    |c| self.cluster_offset(c),
                    self.bpb.cluster_size,
                    size,
                );
                (ext, Condition::Intact)
            } else {
                self.deleted_extents(first_cluster, size)
            };

            if is_dir && self.valid_cluster(first_cluster) {
                subdirs.push((path.clone(), first_cluster, deleted));
            }
            self.entries.push(FileEntry {
                id: self.next_id,
                path,
                name,
                size: if is_dir { 0 } else { size },
                is_dir,
                deleted,
                modified,
                condition,
                integrity: Integrity::Unchecked,
                extents,
            });
            self.next_id += 1;
        }

        for (path, cluster, deleted) in subdirs {
            if !self.visited.insert(cluster) {
                continue;
            }
            let data = if deleted {
                // Catena azzerata: si leggono cluster contigui finché sembrano una directory.
                let clusters: Vec<u32> = (cluster..cluster.saturating_add(64))
                    .take_while(|c| self.valid_cluster(*c))
                    .collect();
                let mut data = self.read_clusters(&clusters);
                if !looks_like_dir(&data) {
                    continue;
                }
                if let Some(end) = data.chunks_exact(32).position(|e| e[0] == 0) {
                    data.truncate(end * 32);
                }
                data
            } else {
                let chain = self.chain(cluster, 65536);
                self.read_clusters(&chain)
            };
            self.walk(&data, &path, deleted, depth + 1);
        }
    }

    /// Extent di un file cancellato: cluster contigui dal primo.
    fn deleted_extents(&mut self, first: u32, size: u64) -> (Vec<super::Extent>, Condition) {
        let n = size.div_ceil(self.bpb.cluster_size) as u32;
        // Se l'ipotesi contigua sfora la fine del volume il file era frammentato:
        // si recupera la parte leggibile (la verifica lo segnalerà come danneggiato).
        let last = first.saturating_add(n).min(self.bpb.cluster_count + 2);
        if last <= first {
            return (Vec::new(), Condition::Unsupported);
        }
        let clusters: Vec<u32> = (first..last).collect();
        // Se uno di quei cluster è oggi allocato, un altro file lo ha riusato.
        let reused = clusters
            .iter()
            .any(|c| self.fat_entry(*c).is_some_and(|v| v != 0));
        let ext = clusters_to_extents(
            &clusters,
            |c| self.cluster_offset(c),
            self.bpb.cluster_size,
            size,
        );
        (
            ext,
            if reused {
                Condition::Overwritten
            } else {
                Condition::Probable
            },
        )
    }
}

/// Le prime due voci di una directory sono "." e "..".
fn looks_like_dir(data: &[u8]) -> bool {
    data.len() >= 64 && &data[0..11] == b".          " && &data[32..34] == b".."
}

fn sfn_checksum(short: &[u8; 11]) -> u8 {
    short
        .iter()
        .fold(0u8, |sum, &c| sum.rotate_right(1).wrapping_add(c))
}

/// Nome lungo (VFAT). Per i cancellati il primo byte del nome corto è 0xE5:
/// si cerca il carattere originale che rende valido il checksum.
fn long_name(parts: &[(u8, [u16; 13])], short: &mut [u8; 11], deleted: bool) -> Option<String> {
    let (checksum, _) = parts.first()?;
    if parts.iter().any(|(c, _)| c != checksum) {
        return None;
    }
    if sfn_checksum(short) != *checksum {
        if !deleted {
            return None;
        }
        let orig = (0x20u8..=0xFF).find(|b| {
            let mut s = *short;
            s[0] = *b;
            sfn_checksum(&s) == *checksum
        })?;
        short[0] = orig;
    }
    let mut bytes = Vec::new();
    for (_, units) in parts.iter().rev() {
        for u in units {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
    }
    let name = utf16le(&bytes);
    (!name.is_empty()).then_some(name)
}

/// Nome 8.3. `nt_case` (byte 12) indica base (0x08) ed estensione (0x10) in minuscolo,
/// come li salvano Windows e mtools. Per i cancellati il primo carattere è perso: `_`.
fn short_name(short: &[u8; 11], deleted: bool, nt_case: u8) -> String {
    let mut s = *short;
    if deleted {
        s[0] = b'_';
    }
    let mut base = String::from_utf8_lossy(&s[0..8]).trim_end().to_string();
    let mut ext = String::from_utf8_lossy(&s[8..11]).trim_end().to_string();
    if nt_case & 0x08 != 0 {
        base = base.to_lowercase();
    }
    if nt_case & 0x10 != 0 {
        ext = ext.to_lowercase();
    }
    if ext.is_empty() {
        base
    } else {
        format!("{base}.{ext}")
    }
}

impl Fat<'_> {
    /// Directory "orfane": cluster che iniziano con le voci "." e ".." e che il
    /// file system attuale non raggiunge (tipicamente dopo una formattazione).
    /// Ogni albero ritrovato viene ricostruito sotto `REBUILT_DIR`.
    /// Ricostruisce gli alberi delle directory orfane trovate (cluster -> padre).
    fn rebuild_orphans(&mut self, found: BTreeMap<u32, u32>) {
        let mut n = 0;
        // Prima le radici degli alberi ritrovati (padre sconosciuto), poi eventuali resti.
        let tops: Vec<u32> = found
            .iter()
            .filter(|(_, p)| !found.contains_key(p))
            .map(|(c, _)| *c)
            .chain(found.keys().copied())
            .collect();
        for top in tops {
            if !self.visited.insert(top) {
                continue;
            }
            let clusters: Vec<u32> = (top..top.saturating_add(64))
                .take_while(|c| self.valid_cluster(*c))
                .collect();
            let mut data = self.read_clusters(&clusters);
            if let Some(end) = data.chunks_exact(32).position(|e| e[0] == 0) {
                data.truncate(end * 32);
            }
            n += 1;
            let path = format!("{}/Cartella ritrovata {n}", super::REBUILT_DIR);
            self.entries.push(FileEntry {
                id: self.next_id,
                path: path.clone(),
                name: format!("Cartella ritrovata {n}"),
                size: 0,
                is_dir: true,
                deleted: true,
                modified: None,
                condition: Condition::Probable,
                integrity: Integrity::Unchecked,
                extents: Vec::new(),
            });
            self.next_id += 1;
            self.walk(&data, &path, true, 1);
        }
    }
}

/// Stato per la ricostruzione: durante la lettura completa del disco si
/// annotano i cluster che iniziano con le voci "." e ".." (directory).
#[derive(Clone)]
pub(crate) struct State {
    bpb: Bpb,
    visited: HashSet<u32>,
    next_id: u64,
    found: BTreeMap<u32, u32>,
}

impl State {
    /// Directory: voce "." che punta al cluster stesso e voce "..". Ritorna il padre.
    fn dir_parent(&self, c: u32, head: &[u8]) -> Option<u32> {
        if head.len() < 64 || &head[0..11] != b".          " || &head[32..43] != b"..         " {
            return None;
        }
        if head[11] & 0x10 == 0 || head[43] & 0x10 == 0 {
            return None;
        }
        let fat32 = self.bpb.fs == FsType::Fat32;
        let cluster_of = |e: &[u8]| {
            let hi = if fat32 {
                u16::from_le_bytes([e[20], e[21]]) as u32
            } else {
                0
            };
            (hi << 16) | u16::from_le_bytes([e[26], e[27]]) as u32
        };
        (cluster_of(&head[0..32]) == c).then(|| cluster_of(&head[32..64]))
    }

    pub(crate) fn feed(&mut self, view: &dyn BlockSource, rel: u64, chunk: &[u8]) {
        let cs = self.bpb.cluster_size;
        let data = self.bpb.data_offset;
        let end = rel + chunk.len() as u64;
        if end <= data {
            return;
        }
        let mut k = (rel.max(data) - data).div_ceil(cs);
        loop {
            let c = 2 + k as u32;
            let off = data + k * cs;
            if off >= end || c >= self.bpb.cluster_count + 2 {
                break;
            }
            let i = (off - rel) as usize;
            let mut tmp = [0u8; 64];
            let head: &[u8] = if i + 64 <= chunk.len() {
                &chunk[i..i + 64]
            } else if crate::source::read_exact_at(view, off, &mut tmp).is_ok() {
                &tmp
            } else {
                &[]
            };
            if let Some(parent) = self.dir_parent(c, head) {
                self.found.insert(c, parent);
            }
            k += 1;
        }
    }

    pub(crate) fn finish(self, view: &dyn BlockSource) -> Vec<FileEntry> {
        let mut fat = Fat {
            bpb: self.bpb,
            r: ByteReader::new(view),
            next_id: self.next_id,
            entries: Vec::new(),
            visited: self.visited,
        };
        fat.rebuild_orphans(self.found);
        fat.entries
    }
}

pub fn scan(src: &dyn BlockSource, vol: &mut Volume) -> Result<(), String> {
    let mut b = vec![0u8; 512];
    crate::source::read_exact_at(src, 0, &mut b).map_err(|e| e.to_string())?;
    let bpb = Bpb::parse(&b).ok_or("settore di avvio FAT non valido")?;
    vol.fs = Some(bpb.fs);
    vol.cluster_size = bpb.cluster_size as u32;
    vol.label = bpb.label.clone();

    let mut fat = Fat {
        bpb,
        r: ByteReader::new(src),
        next_id: 1,
        entries: Vec::new(),
        visited: HashSet::new(),
    };
    let root = fat.root_data();
    // Etichetta del volume dalla directory radice, se presente.
    if let Some(e) = root
        .chunks_exact(32)
        .take_while(|e| e[0] != 0)
        .find(|e| e[11] & 0x08 != 0 && e[11] != 0x0F && e[0] != 0xE5)
    {
        let label = String::from_utf8_lossy(&e[0..11]).trim().to_string();
        if !label.is_empty() {
            vol.label = Some(label);
        }
    }
    fat.walk(&root, "", false, 0);
    vol.state = Some(super::FsState::Fat(State {
        bpb: fat.bpb.clone(),
        visited: std::mem::take(&mut fat.visited),
        next_id: fat.next_id,
        found: BTreeMap::new(),
    }));
    vol.entries = fat.entries;
    Ok(())
}
