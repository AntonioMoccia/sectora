//! Tabelle delle partizioni MBR (con partizioni logiche) e GPT.

use crate::source::{read_exact_at, BlockSource};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Partition {
    /// 1-based, nell'ordine della tabella.
    pub index: u32,
    pub offset: u64,
    pub size: u64,
    /// Tipo leggibile, es. "MBR 0x0C FAT32 LBA" o "GPT Microsoft basic data".
    pub kind: String,
}

pub fn read_partitions(src: &dyn BlockSource) -> Vec<Partition> {
    let mut mbr = [0u8; 512];
    if read_exact_at(src, 0, &mut mbr).is_err() || mbr[510..512] != [0x55, 0xAA] {
        // Senza MBR può esserci comunque un GPT (raro, ma succede con dischi mal inizializzati).
        return read_gpt(src).unwrap_or_default();
    }
    let entries = mbr_entries(&mbr, 0);
    if entries.iter().any(|e| e.kind == 0xEE) {
        if let Some(gpt) = read_gpt(src) {
            return gpt;
        }
    }

    let size = src.size();
    let mut out = Vec::new();
    let mut index = 1;
    for e in entries {
        if e.kind == 0 || e.count == 0 {
            continue;
        }
        if matches!(e.kind, 0x05 | 0x0F | 0x85) {
            read_extended(src, e.start, &mut out, &mut index, size);
            continue;
        }
        push(&mut out, &mut index, e.start, e.count, e.kind, size);
    }
    out
}

struct MbrEntry {
    kind: u8,
    start: u64,
    count: u64,
}

fn mbr_entries(sector: &[u8; 512], base_lba: u64) -> Vec<MbrEntry> {
    (0..4)
        .filter_map(|i| {
            let e = &sector[446 + i * 16..446 + (i + 1) * 16];
            if e[0] != 0x00 && e[0] != 0x80 {
                return None;
            }
            Some(MbrEntry {
                kind: e[4],
                start: base_lba + u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as u64,
                count: u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as u64,
            })
        })
        .collect()
}

fn push(out: &mut Vec<Partition>, index: &mut u32, start: u64, count: u64, kind: u8, size: u64) {
    let offset = start * 512;
    if start == 0 || offset >= size {
        return;
    }
    out.push(Partition {
        index: *index,
        offset,
        size: (count * 512).min(size - offset),
        kind: format!("MBR 0x{kind:02X} {}", mbr_type_name(kind)),
    });
    *index += 1;
}

/// Catena di EBR: ogni partizione logica ha il suo settore con 2 voci.
fn read_extended(
    src: &dyn BlockSource,
    ext_start: u64,
    out: &mut Vec<Partition>,
    index: &mut u32,
    size: u64,
) {
    let mut ebr_lba = ext_start;
    for _ in 0..128 {
        let mut sector = [0u8; 512];
        if read_exact_at(src, ebr_lba * 512, &mut sector).is_err()
            || sector[510..512] != [0x55, 0xAA]
        {
            return;
        }
        let entries = mbr_entries(&sector, 0);
        let Some(first) = entries.first() else { return };
        if first.kind != 0 && first.count != 0 {
            push(
                out,
                index,
                ebr_lba + first.start,
                first.count,
                first.kind,
                size,
            );
        }
        match entries.get(1) {
            Some(next) if next.kind != 0 && next.start != 0 => ebr_lba = ext_start + next.start,
            _ => return,
        }
    }
}

fn mbr_type_name(kind: u8) -> &'static str {
    match kind {
        0x01 => "FAT12",
        0x04 | 0x06 | 0x0E => "FAT16",
        0x0B | 0x0C => "FAT32",
        0x07 => "NTFS/exFAT",
        0x83 => "Linux",
        0xAF => "HFS+",
        0xEE => "GPT protettivo",
        _ => "",
    }
}

fn read_gpt(src: &dyn BlockSource) -> Option<Vec<Partition>> {
    for sector in [512u64, 4096] {
        let mut hdr = [0u8; 92];
        if read_exact_at(src, sector, &mut hdr).is_err() || &hdr[0..8] != b"EFI PART" {
            continue;
        }
        let entries_lba = u64::from_le_bytes(hdr[72..80].try_into().ok()?);
        let count = u32::from_le_bytes(hdr[80..84].try_into().ok()?).min(1024) as u64;
        let entry_size = u32::from_le_bytes(hdr[84..88].try_into().ok()?) as u64;
        if entry_size < 128 {
            return None;
        }
        let mut table = vec![0u8; (count * entry_size) as usize];
        read_exact_at(src, entries_lba * sector, &mut table).ok()?;
        let mut out = Vec::new();
        for (i, e) in table.chunks_exact(entry_size as usize).enumerate() {
            let type_guid = &e[0..16];
            if type_guid.iter().all(|b| *b == 0) {
                continue;
            }
            let first = u64::from_le_bytes(e[32..40].try_into().ok()?);
            let last = u64::from_le_bytes(e[40..48].try_into().ok()?);
            if last < first {
                continue;
            }
            let name = super::utf16le(&e[56..128]);
            let guid = format_guid(type_guid);
            let label = match gpt_type_name(&guid) {
                Some(t) if !name.is_empty() => format!("GPT {t} \"{name}\""),
                Some(t) => format!("GPT {t}"),
                None => format!("GPT {guid}"),
            };
            let offset = first * sector;
            if offset >= src.size() {
                continue;
            }
            out.push(Partition {
                index: i as u32 + 1,
                offset,
                size: ((last - first + 1) * sector).min(src.size() - offset),
                kind: label,
            });
        }
        return Some(out);
    }
    None
}

/// GUID in formato testuale: i primi tre campi sono little-endian.
fn format_guid(g: &[u8]) -> String {
    format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        g[3], g[2], g[1], g[0], g[5], g[4], g[7], g[6], g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
    )
}

fn gpt_type_name(guid: &str) -> Option<&'static str> {
    Some(match guid {
        "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7" => "Microsoft basic data",
        "C12A7328-F81F-11D2-BA4B-00A0C93EC93B" => "EFI System",
        "E3C9E316-0B5C-4DB8-817D-F92DF00215AE" => "Microsoft reserved",
        "DE94BBA4-06D1-4D40-A16A-BFD50179D6AC" => "Windows recovery",
        "7C3457EF-0000-11AA-AA11-00306543ECAC" => "APFS",
        "48465300-0000-11AA-AA11-00306543ECAC" => "HFS+",
        "0FC63DAF-8483-4772-8E79-3D69D8477DE4" => "Linux",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemorySource;

    #[test]
    fn mbr_with_logical_partitions() {
        let mut disk = vec![0u8; 4096 * 512];
        let set =
            |d: &mut Vec<u8>, sector: usize, slot: usize, kind: u8, start: u32, count: u32| {
                let o = sector * 512 + 446 + slot * 16;
                d[o + 4] = kind;
                d[o + 8..o + 12].copy_from_slice(&start.to_le_bytes());
                d[o + 12..o + 16].copy_from_slice(&count.to_le_bytes());
                d[sector * 512 + 510] = 0x55;
                d[sector * 512 + 511] = 0xAA;
            };
        set(&mut disk, 0, 0, 0x0C, 2048, 1000);
        set(&mut disk, 0, 1, 0x0F, 3100, 900); // estesa
        set(&mut disk, 3100, 0, 0x07, 63, 300); // logica 1
        set(&mut disk, 3100, 1, 0x05, 400, 400); // link al prossimo EBR
        set(&mut disk, 3500, 0, 0x83, 63, 200); // logica 2
        let src = MemorySource::new(disk, "mbr");
        let parts = read_partitions(&src);
        let got: Vec<(u64, u64)> = parts
            .iter()
            .map(|p| (p.offset / 512, p.size / 512))
            .collect();
        assert_eq!(got, vec![(2048, 1000), (3163, 300), (3563, 200)]);
    }

    #[test]
    fn gpt_partitions() {
        let mut disk = vec![0u8; 8192 * 512];
        // MBR protettivo.
        disk[446 + 4] = 0xEE;
        disk[446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
        disk[446 + 12..446 + 16].copy_from_slice(&8191u32.to_le_bytes());
        disk[510] = 0x55;
        disk[511] = 0xAA;
        // Header GPT a LBA 1, voci a LBA 2.
        let h = 512;
        disk[h..h + 8].copy_from_slice(b"EFI PART");
        disk[h + 72..h + 80].copy_from_slice(&2u64.to_le_bytes());
        disk[h + 80..h + 84].copy_from_slice(&128u32.to_le_bytes());
        disk[h + 84..h + 88].copy_from_slice(&128u32.to_le_bytes());
        let basic = [
            0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26,
            0x99, 0xC7,
        ];
        let e = 1024 + 128; // seconda voce (la prima è vuota)
        disk[e..e + 16].copy_from_slice(&basic);
        disk[e + 32..e + 40].copy_from_slice(&2048u64.to_le_bytes());
        disk[e + 40..e + 48].copy_from_slice(&6143u64.to_le_bytes());
        for (i, c) in "Dati".encode_utf16().enumerate() {
            disk[e + 56 + i * 2..e + 58 + i * 2].copy_from_slice(&c.to_le_bytes());
        }
        let parts = read_partitions(&MemorySource::new(disk, "gpt"));
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].index, 2);
        assert_eq!(parts[0].offset, 2048 * 512);
        assert_eq!(parts[0].size, 4096 * 512);
        assert_eq!(parts[0].kind, "GPT Microsoft basic data \"Dati\"");
    }

    #[test]
    fn guid_format() {
        let raw = [
            0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26,
            0x99, 0xC7,
        ];
        assert_eq!(format_guid(&raw), "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7");
    }
}
