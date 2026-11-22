//! Robustezza: il Quick Scan non deve mai andare in panic su volumi
//! danneggiati. Si parte da settori di avvio validi e si corrompono byte a caso.

use sectora_engine::{quick_scan, MemorySource};

fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

fn boot_sector(kind: &str) -> Vec<u8> {
    let mut b = vec![0u8; 512];
    b[0] = 0xEB;
    b[510] = 0x55;
    b[511] = 0xAA;
    match kind {
        "ntfs" => {
            b[3..11].copy_from_slice(b"NTFS    ");
            b[11..13].copy_from_slice(&512u16.to_le_bytes());
            b[13] = 8;
            b[48..56].copy_from_slice(&4u64.to_le_bytes());
            b[64] = 0xF6; // record da 1024 byte
        }
        "exfat" => {
            b[3..11].copy_from_slice(b"EXFAT   ");
            b[80..84].copy_from_slice(&128u32.to_le_bytes());
            b[88..92].copy_from_slice(&256u32.to_le_bytes());
            b[92..96].copy_from_slice(&1000u32.to_le_bytes());
            b[96..100].copy_from_slice(&4u32.to_le_bytes());
            b[108] = 9;
            b[109] = 3;
        }
        _ => {
            b[11..13].copy_from_slice(&512u16.to_le_bytes());
            b[13] = 4;
            b[14..16].copy_from_slice(&32u16.to_le_bytes());
            b[16] = 2;
            b[32..36].copy_from_slice(&200_000u32.to_le_bytes());
            b[36..40].copy_from_slice(&200u32.to_le_bytes());
            b[44..48].copy_from_slice(&2u32.to_le_bytes());
        }
    }
    b
}

#[test]
fn corrupted_volumes_never_panic() {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    for kind in ["ntfs", "exfat", "fat32"] {
        for round in 0..150 {
            let mut disk: Vec<u8> = (0..2 * 1024 * 1024).map(|_| rng(&mut seed) as u8).collect();
            disk[..512].copy_from_slice(&boot_sector(kind));
            // Strutture plausibili ripetute (record "FILE", voci di directory), poi rumore.
            for i in (4096..disk.len()).step_by(1024) {
                if rng(&mut seed) % 3 == 0 {
                    disk[i..i + 4].copy_from_slice(b"FILE");
                    disk[i + 4..i + 6].copy_from_slice(&48u16.to_le_bytes());
                    disk[i + 6..i + 8].copy_from_slice(&3u16.to_le_bytes());
                }
                if rng(&mut seed) % 4 == 0 {
                    disk[i] = [0x85, 0x05, 0xE5, 0x41][(round + i) % 4];
                }
            }
            let src = MemorySource::new(disk, "corrotto");
            let _ = quick_scan(&src);
        }
    }
}
