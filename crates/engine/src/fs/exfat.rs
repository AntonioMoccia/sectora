//! exFAT (schede SD > 32 GB, chiavette, dischi esterni condivisi Mac/Windows).
//!
//! Ogni file è un "entry set": voce File (0x85), voce Stream (0xC0) e una o
//! più voci Nome (0xC1). Alla cancellazione viene azzerato il bit "in uso"
//! di ogni voce (0x05, 0x40, 0x41) ma nome, dimensione e primo cluster
//! restano. Se il file era contiguo (flag NoFatChain, il caso più comune) la
//! posizione dei dati è nota con certezza.

use super::{
    clusters_to_extents, dos_datetime, fragment_owner, utf16le, Condition, DirTail, Extent,
    FileEntry, FsType, Integrity, Volume,
};
use crate::carve::reader::ByteReader;
use crate::source::BlockSource;
use std::collections::HashSet;

#[derive(Clone)]
struct Geometry {
    fat_offset: u64,
    heap_offset: u64,
    cluster_size: u64,
    cluster_count: u32,
    root_cluster: u32,
}

struct ExFat<'a> {
    g: Geometry,
    r: ByteReader<'a>,
    next_id: u64,
    entries: Vec<FileEntry>,
    visited: HashSet<u32>,
    label: Option<String>,
    bitmap: Option<Vec<u8>>,
    tails: Vec<DirTail>,
    /// Primi cluster di directory note: la lettura contigua di un elenco si ferma lì.
    dir_starts: HashSet<u32>,
}

/// Il cluster può essere la continuazione di un elenco exFAT (gli entry set
/// possono anche iniziare nel cluster precedente).
fn continues_dir(d: &[u8]) -> bool {
    d.len() >= 32 && matches!(d[0], 0x85 | 0x05 | 0xC0 | 0x40 | 0xC1 | 0x41)
}

fn has_end(d: &[u8]) -> bool {
    d.chunks_exact(32).any(|e| e[0] == 0)
}

impl ExFat<'_> {
    /// Elenco di una directory cancellata o ritrovata, a partire da `first`:
    /// cluster contigui finché sembrano continuare l'elenco (al massimo `max`).
    /// Ritorna dati, cluster letti e se l'elenco termina lì.
    fn read_dir_contig(&mut self, first: u32, max: u32) -> Option<(Vec<u8>, Vec<u32>, bool)> {
        let mut data = self.read_clusters(&[first]);
        if data.is_empty() {
            return None;
        }
        let mut clusters = vec![first];
        loop {
            if has_end(&data) {
                return Some((data, clusters, true));
            }
            let next = clusters[clusters.len() - 1] + 1;
            if clusters.len() as u32 >= max
                || !self.valid(next)
                || self.visited.contains(&next)
                || self.dir_starts.contains(&next)
            {
                break;
            }
            let d = self.read_clusters(&[next]);
            if !continues_dir(&d) {
                break;
            }
            data.extend_from_slice(&d);
            clusters.push(next);
        }
        Some((data, clusters, false))
    }

    fn cluster_offset(&self, c: u32) -> u64 {
        self.g.heap_offset + (c as u64 - 2) * self.g.cluster_size
    }

    fn valid(&self, c: u32) -> bool {
        c >= 2 && c < self.g.cluster_count + 2
    }

    fn chain(&mut self, first: u32, max: usize) -> Vec<u32> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut c = first;
        while self.valid(c) && out.len() < max && seen.insert(c) {
            out.push(c);
            match self.r.u32_le(self.g.fat_offset + c as u64 * 4) {
                Some(v) if (2..0xFFFF_FFF7).contains(&v) => c = v,
                _ => break,
            }
        }
        out
    }

    fn allocated(&self, c: u32) -> bool {
        let Some(bm) = &self.bitmap else { return false };
        let i = (c - 2) as usize;
        bm.get(i / 8).is_some_and(|b| b & (1 << (i % 8)) != 0)
    }

    /// Cluster di uno stream: contigui se NoFatChain, altrimenti dalla FAT.
    fn stream_clusters(&mut self, first: u32, len: u64, no_fat_chain: bool) -> Option<Vec<u32>> {
        if !self.valid(first) || len == 0 {
            return None;
        }
        let n = len.div_ceil(self.g.cluster_size) as u32;
        if no_fat_chain {
            let last = first.checked_add(n)?;
            if !self.valid(last - 1) {
                return None;
            }
            return Some((first..last).collect());
        }
        let chain = self.chain(first, n as usize);
        (chain.len() == n as usize).then_some(chain)
    }

    fn read_clusters(&mut self, clusters: &[u32]) -> Vec<u8> {
        let cs = self.g.cluster_size as usize;
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

    /// `tail`: ultimo cluster dell'elenco letto e cluster che mancano ancora
    /// (solo per le directory ricostruite).
    fn walk(
        &mut self,
        data: &[u8],
        parent: &str,
        parent_deleted: bool,
        depth: u32,
        tail: Option<(u32, u32)>,
    ) {
        if depth > 64 {
            return;
        }
        let start = self.entries.len();
        let mut pending = None;
        let entries: Vec<&[u8]> = data.chunks_exact(32).collect();
        let mut subdirs = Vec::new();
        let mut i = 0;
        while i < entries.len() {
            let e = entries[i];
            let typ = e[0];
            if typ == 0x00 {
                break;
            }
            match typ {
                0x83 if depth == 0 => {
                    let n = (e[1] as usize).min(11);
                    self.label = Some(utf16le(&e[2..2 + n * 2]));
                }
                0x81 if depth == 0 && self.bitmap.is_none() => {
                    let first = u32::from_le_bytes(e[20..24].try_into().unwrap());
                    let len = u64::from_le_bytes(e[24..32].try_into().unwrap());
                    if let Some(cl) = self.stream_clusters(first, len.min(512 << 20), false) {
                        let mut bm = self.read_clusters(&cl);
                        bm.truncate(len as usize);
                        self.bitmap = Some(bm);
                    }
                }
                0x85 | 0x05 => {
                    let secondary = e[1] as usize;
                    if (2..=18).contains(&secondary) && i + secondary >= entries.len() {
                        pending = Some(i * 32);
                    }
                    if secondary >= 2 && i + secondary < entries.len() {
                        if let Some(sub) =
                            self.file_set(&entries[i..=i + secondary], parent, parent_deleted)
                        {
                            subdirs.push(sub);
                        }
                        i += secondary + 1;
                        continue;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        if let Some((last, missing)) = tail {
            let mut t = DirTail::new(
                parent,
                last,
                missing,
                &self.entries[start..],
                self.g.heap_offset,
                self.g.cluster_size,
            );
            if let Some(p) = pending {
                t.pending = data[p..].to_vec();
            }
            self.tails.push(t);
        }

        self.dir_starts.extend(subdirs.iter().map(|s| s.1));
        for (path, first, len, no_fat, deleted) in subdirs {
            if !self.visited.insert(first) {
                continue;
            }
            let len = len.min(64 << 20);
            if deleted {
                // Catena FAT persa (cancellazione o formattazione): cluster contigui
                // finché continuano l'elenco; quelli mancanti si cercano dopo.
                let n = len.div_ceil(self.g.cluster_size).max(1) as u32;
                let Some((data, clusters, ended)) = self.read_dir_contig(first, n) else {
                    continue;
                };
                let last = *clusters.last().unwrap();
                let missing = if ended {
                    0
                } else {
                    n.saturating_sub(clusters.len() as u32)
                };
                self.visited.extend(clusters.iter().copied());
                self.walk(&data, &path, true, depth + 1, Some((last, missing)));
                continue;
            }
            let Some(clusters) = self.stream_clusters(first, len, no_fat) else {
                continue;
            };
            self.visited.extend(clusters.iter().copied());
            let data = self.read_clusters(&clusters);
            self.walk(&data, &path, false, depth + 1, None);
        }
    }

    /// Legge un entry set. Ritorna i dati per visitare la sottodirectory, se lo è.
    fn file_set(
        &mut self,
        set: &[&[u8]],
        parent: &str,
        parent_deleted: bool,
    ) -> Option<(String, u32, u64, bool, bool)> {
        let file = set[0];
        let stream = set[1];
        let in_use = file[0] == 0x85;
        // Voci coerenti: tutte in uso o tutte cancellate.
        let stream_ok = (in_use && stream[0] == 0xC0) || (!in_use && stream[0] == 0x40);
        if !stream_ok {
            return None;
        }
        let deleted = !in_use || parent_deleted;
        let attrs = u16::from_le_bytes([file[4], file[5]]);
        let is_dir = attrs & 0x10 != 0;
        let ts = u32::from_le_bytes(file[12..16].try_into().ok()?);
        let modified = dos_datetime((ts >> 16) as u16, ts as u16);

        let flags = stream[1];
        let no_fat_chain = flags & 0x02 != 0;
        let name_len = stream[3] as usize;
        let first = u32::from_le_bytes(stream[20..24].try_into().ok()?);
        let size = u64::from_le_bytes(stream[24..32].try_into().ok()?);

        let mut name_bytes = Vec::new();
        for n in &set[2..] {
            if n[0] & 0x7F != 0x41 {
                break;
            }
            name_bytes.extend_from_slice(&n[2..32]);
        }
        name_bytes.truncate(name_len * 2);
        let name = utf16le(&name_bytes);
        if name.is_empty() {
            return None;
        }
        let path = format!("{parent}/{name}");

        let (extents, condition) = if is_dir || size == 0 {
            (
                Vec::new(),
                if deleted {
                    Condition::Recoverable
                } else {
                    Condition::Intact
                },
            )
        } else {
            match self.stream_clusters(first, size, no_fat_chain) {
                Some(cl) => {
                    let reused = deleted && cl.iter().any(|c| self.allocated(*c));
                    let ext: Vec<Extent> = clusters_to_extents(
                        &cl,
                        |c| self.cluster_offset(c),
                        self.g.cluster_size,
                        size,
                    );
                    let cond = match (deleted, reused, no_fat_chain) {
                        (false, _, _) => Condition::Intact,
                        (true, true, _) => Condition::Overwritten,
                        (true, false, true) => Condition::Recoverable,
                        (true, false, false) => Condition::Probable,
                    };
                    (ext, cond)
                }
                None if deleted => {
                    // Catena FAT persa: si prova l'ipotesi contigua.
                    match self.stream_clusters(first, size, true) {
                        Some(cl) => (
                            clusters_to_extents(
                                &cl,
                                |c| self.cluster_offset(c),
                                self.g.cluster_size,
                                size,
                            ),
                            Condition::Probable,
                        ),
                        None => (Vec::new(), Condition::Unsupported),
                    }
                }
                None => (Vec::new(), Condition::Unsupported),
            }
        };

        self.entries.push(FileEntry {
            id: self.next_id,
            path: path.clone(),
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
        is_dir.then_some((path, first, size, no_fat_chain, deleted))
    }
}

/// Checksum di un entry set (specifica exFAT, 6.3.3). Per i set cancellati si
/// ripristina il bit "in uso" prima del calcolo.
fn set_checksum(set: &[&[u8]]) -> u16 {
    let mut sum: u16 = 0;
    for (i, e) in set.iter().enumerate() {
        for (j, &b) in e.iter().enumerate() {
            if i == 0 && (j == 2 || j == 3) {
                continue;
            }
            let b = if j == 0 { b | 0x80 } else { b };
            sum = (if sum & 1 != 0 { 0x8000u16 } else { 0 })
                .wrapping_add(sum >> 1)
                .wrapping_add(b as u16);
        }
    }
    sum
}

/// `true` se il cluster inizia con un entry set di file valido.
fn starts_with_file_set(data: &[u8]) -> bool {
    if data.len() < 96 || !matches!(data[0], 0x85 | 0x05) {
        return false;
    }
    let secondary = data[1] as usize;
    if !(2..=18).contains(&secondary) || data.len() < (secondary + 1) * 32 {
        return false;
    }
    if !matches!(data[32], 0xC0 | 0x40) || !matches!(data[64], 0xC1 | 0x41) {
        return false;
    }
    let set: Vec<&[u8]> = data[..(secondary + 1) * 32].chunks_exact(32).collect();
    set_checksum(&set) == u16::from_le_bytes([data[2], data[3]])
}

/// Nome del primo file completo del blocco.
fn exfat_first_name(d: &[u8]) -> Option<String> {
    let entries: Vec<&[u8]> = d.chunks_exact(32).collect();
    let i = entries.iter().position(|e| matches!(e[0], 0x85 | 0x05))?;
    let stream = entries.get(i + 1)?;
    let len = stream[3] as usize;
    let mut bytes = Vec::new();
    for e in entries.iter().skip(i + 2).take(17) {
        if e[0] & 0x7F != 0x41 {
            break;
        }
        bytes.extend_from_slice(&e[2..32]);
    }
    bytes.truncate(len * 2);
    let name = utf16le(&bytes);
    (!name.is_empty()).then_some(name)
}

/// Quanto il blocco `frag` continua l'elenco di `t` (vedi `fragment_owner`).
fn exfat_fit(t: &DirTail, frag: &[u8]) -> u8 {
    let starts_clean = frag.first().is_some_and(|b| matches!(b, 0x85 | 0x05));
    if t.pending.is_empty() {
        return u8::from(starts_clean);
    }
    let mut joined = t.pending.clone();
    joined.extend_from_slice(&frag[..frag.len().min(640)]);
    if starts_with_file_set(&joined) {
        2
    } else {
        0
    }
}

/// Il cluster inizia con la parte finale di un entry set (iniziato nel cluster
/// precedente dell'elenco) seguita da un entry set completo e valido.
fn continues_file_sets(data: &[u8]) -> bool {
    if data.len() < 32 || !matches!(data[0], 0xC0 | 0x40 | 0xC1 | 0x41) {
        return false;
    }
    for (k, e) in data.chunks_exact(32).enumerate().take(19) {
        match e[0] {
            0xC0 | 0x40 | 0xC1 | 0x41 => continue,
            0x85 | 0x05 => return starts_with_file_set(&data[k * 32..]),
            _ => return false,
        }
    }
    false
}

impl ExFat<'_> {
    /// Directory del file system precedente non raggiungibili dall'attuale
    /// (dopo una formattazione), ricostruite sotto `REBUILT_DIR`. I candidati non
    /// referenziati da altri sono le cartelle di primo livello (il loro nome stava
    /// nella radice, sovrascritta) oppure i blocchi successivi dell'elenco di una
    /// cartella già ricostruita: questi vengono riattaccati alla cartella giusta.
    fn rebuild_orphans(&mut self, mut candidates: Vec<u32>) {
        candidates.retain(|c| !self.visited.contains(c));
        candidates.sort_unstable();
        candidates.dedup();
        let cs = self.g.cluster_size as usize;
        let mut referenced = HashSet::new();
        for &c in &candidates {
            let off = self.cluster_offset(c);
            let mut data = vec![0u8; cs];
            if self.r.read(off, &mut data).is_none() {
                continue;
            }
            for e in data.chunks_exact(32) {
                if e[0] == 0 {
                    break;
                }
                if matches!(e[0], 0xC0 | 0x40) {
                    referenced.insert(u32::from_le_bytes(e[20..24].try_into().unwrap()));
                }
            }
        }
        self.dir_starts.extend(referenced.iter().copied());
        // In ordine di posizione sul disco, cioè di scrittura: la cartella a cui
        // appartiene un blocco è sempre già stata ricostruita quando lo si incontra.
        let order: Vec<u32> = candidates
            .iter()
            .filter(|c| !referenced.contains(c))
            .chain(candidates.iter())
            .copied()
            .collect();
        let mut n = 0;
        for top in order {
            if self.visited.contains(&top) {
                continue;
            }
            let Some((data, clusters, ended)) = self.read_dir_contig(top, 16) else {
                continue;
            };
            self.visited.extend(clusters.iter().copied());
            let last = *clusters.last().unwrap();
            let first = exfat_first_name(&data);
            if let Some(i) =
                fragment_owner(&self.tails, top, first.as_deref(), |t| exfat_fit(t, &data))
            {
                let owner = &mut self.tails[i];
                let rest = owner.missing.saturating_sub(clusters.len() as u32);
                owner.missing = 0;
                let path = owner.path.clone();
                let mut joined = std::mem::take(&mut owner.pending);
                joined.extend_from_slice(&data);
                let missing = if ended { 0 } else { rest };
                self.walk(&joined, &path, true, 1, Some((last, missing)));
                continue;
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
            // Lunghezza sconosciuta: se l'elenco non termina, può continuare altrove.
            self.walk(&data, &path, true, 1, Some((last, u32::from(!ended))));
        }
    }
}

/// Stato per la ricostruzione: durante la lettura completa si annotano i
/// cluster che iniziano con un entry set valido (probabili directory).
#[derive(Clone)]
pub(crate) struct State {
    g: Geometry,
    visited: HashSet<u32>,
    next_id: u64,
    bitmap: Option<Vec<u8>>,
    candidates: Vec<u32>,
    tails: Vec<DirTail>,
}

impl State {
    pub(crate) fn feed(&mut self, view: &dyn BlockSource, rel: u64, chunk: &[u8]) {
        const HEAD: usize = 1280; // due entry set lunghi: 19 voci da 32 byte ciascuno
        let cs = self.g.cluster_size;
        let heap = self.g.heap_offset;
        let end = rel + chunk.len() as u64;
        if end <= heap {
            return;
        }
        let mut k = (rel.max(heap) - heap).div_ceil(cs);
        loop {
            let c = 2 + k as u32;
            let off = heap + k * cs;
            if off >= end || c >= self.g.cluster_count + 2 {
                break;
            }
            let i = (off - rel) as usize;
            let want = HEAD.min(cs as usize);
            let mut tmp = vec![0u8; want];
            let head: &[u8] = if i + want <= chunk.len() {
                &chunk[i..i + want]
            } else if crate::source::read_exact_at(view, off, &mut tmp).is_ok() {
                &tmp
            } else {
                &[]
            };
            if !self.visited.contains(&c)
                && (starts_with_file_set(head) || continues_file_sets(head))
            {
                self.candidates.push(c);
            }
            k += 1;
        }
    }

    pub(crate) fn finish(self, view: &dyn BlockSource) -> Vec<FileEntry> {
        let mut fs = ExFat {
            g: self.g,
            r: ByteReader::new(view),
            next_id: self.next_id,
            entries: Vec::new(),
            visited: self.visited,
            label: None,
            bitmap: self.bitmap,
            tails: self.tails,
            dir_starts: HashSet::new(),
        };
        fs.rebuild_orphans(self.candidates);
        fs.entries
    }
}

pub fn scan(src: &dyn BlockSource, vol: &mut Volume) -> Result<(), String> {
    let mut b = vec![0u8; 512];
    crate::source::read_exact_at(src, 0, &mut b).map_err(|e| e.to_string())?;
    if &b[3..11] != b"EXFAT   " {
        return Err("settore di avvio exFAT non valido".into());
    }
    let le32 = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let sector_shift = b[108] as u32;
    let cluster_shift = b[109] as u32;
    if !(9..=12).contains(&sector_shift) || sector_shift + cluster_shift > 25 {
        return Err("geometria exFAT non valida".into());
    }
    let sector = 1u64 << sector_shift;
    let g = Geometry {
        fat_offset: le32(80) as u64 * sector,
        heap_offset: le32(88) as u64 * sector,
        cluster_size: sector << cluster_shift,
        cluster_count: le32(92),
        root_cluster: le32(96),
    };
    vol.fs = Some(FsType::ExFat);
    vol.cluster_size = g.cluster_size as u32;

    let mut fs = ExFat {
        g,
        r: ByteReader::new(src),
        next_id: 1,
        entries: Vec::new(),
        visited: HashSet::new(),
        label: None,
        bitmap: None,
        tails: Vec::new(),
        dir_starts: HashSet::new(),
    };
    let root_chain = fs.chain(fs.g.root_cluster, 4096);
    if root_chain.is_empty() {
        return Err("directory radice exFAT non trovata".into());
    }
    let root = fs.read_clusters(&root_chain);
    fs.visited.extend(root_chain.iter().copied());
    fs.walk(&root, "", false, 0, None);
    vol.label = fs.label.take().filter(|l| !l.is_empty());
    vol.state = Some(super::FsState::ExFat(State {
        g: fs.g.clone(),
        visited: std::mem::take(&mut fs.visited),
        next_id: fs.next_id,
        bitmap: fs.bitmap.take(),
        candidates: Vec::new(),
        tails: std::mem::take(&mut fs.tails),
    }));
    vol.entries = fs.entries;
    Ok(())
}
