//! Firme dei formati e parser che determinano la lunghezza esatta del file.
//!
//! Ogni parser cammina la struttura del formato invece di cercare solo il
//! "footer": così una miniatura EXIF dentro un JPEG non tronca il file, e
//! un archivio ZIP annidato non viene confuso con quello esterno.
//!
//! Limite noto: tutti i parser assumono file contigui (non frammentati).

use super::reader::ByteReader;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Jpeg,
    Png,
    Gif,
    Pdf,
    Zip,
    Docx,
    Xlsx,
    Pptx,
    Mp4,
    Mov,
    Heic,
}

impl FileKind {
    pub const ALL: [FileKind; 11] = [
        FileKind::Jpeg,
        FileKind::Png,
        FileKind::Gif,
        FileKind::Pdf,
        FileKind::Zip,
        FileKind::Docx,
        FileKind::Xlsx,
        FileKind::Pptx,
        FileKind::Mp4,
        FileKind::Mov,
        FileKind::Heic,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            FileKind::Jpeg => "jpg",
            FileKind::Png => "png",
            FileKind::Gif => "gif",
            FileKind::Pdf => "pdf",
            FileKind::Zip => "zip",
            FileKind::Docx => "docx",
            FileKind::Xlsx => "xlsx",
            FileKind::Pptx => "pptx",
            FileKind::Mp4 => "mp4",
            FileKind::Mov => "mov",
            FileKind::Heic => "heic",
        }
    }

    /// Categoria usata per filtri e cartelle di output.
    pub fn category(self) -> &'static str {
        match self {
            FileKind::Jpeg | FileKind::Png | FileKind::Gif | FileKind::Heic => "foto",
            FileKind::Mp4 | FileKind::Mov => "video",
            FileKind::Pdf | FileKind::Docx | FileKind::Xlsx | FileKind::Pptx => "documenti",
            FileKind::Zip => "archivi",
        }
    }

    pub fn from_extension(ext: &str) -> Option<FileKind> {
        let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|k| k.extension() == ext || (ext == "jpeg" && *k == FileKind::Jpeg))
    }
}

/// Famiglia di firma: più `FileKind` possono condividere la stessa firma
/// (ZIP → docx/xlsx/pptx, ISO-BMFF → mp4/mov/heic) e il tipo finale viene
/// deciso dal parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Jpeg,
    Png,
    Gif,
    Pdf,
    Zip,
    IsoBmff,
}

impl Family {
    pub fn of(kind: FileKind) -> Family {
        match kind {
            FileKind::Jpeg => Family::Jpeg,
            FileKind::Png => Family::Png,
            FileKind::Gif => Family::Gif,
            FileKind::Pdf => Family::Pdf,
            FileKind::Zip | FileKind::Docx | FileKind::Xlsx | FileKind::Pptx => Family::Zip,
            FileKind::Mp4 | FileKind::Mov | FileKind::Heic => Family::IsoBmff,
        }
    }

    /// Dimensione massima accettata, per limitare il costo dei falsi positivi.
    fn max_size(self) -> u64 {
        const MIB: u64 = 1024 * 1024;
        match self {
            Family::Jpeg | Family::Png => 128 * MIB,
            Family::Gif => 64 * MIB,
            Family::Pdf => 512 * MIB,
            Family::Zip => 2048 * MIB,
            Family::IsoBmff => 256 * 1024 * MIB,
        }
    }
}

/// Riconosce la firma all'inizio di `h` (almeno 16 byte se disponibili).
pub fn detect(h: &[u8]) -> Option<Family> {
    if h.len() >= 4 && h[0] == 0xFF && h[1] == 0xD8 && h[2] == 0xFF && h[3] >= 0xC0 {
        return Some(Family::Jpeg);
    }
    if h.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Family::Png);
    }
    if h.starts_with(b"GIF87a") || h.starts_with(b"GIF89a") {
        return Some(Family::Gif);
    }
    if h.starts_with(b"%PDF-1.") || h.starts_with(b"%PDF-2.") {
        return Some(Family::Pdf);
    }
    if h.starts_with(b"PK\x03\x04") {
        return Some(Family::Zip);
    }
    if h.len() >= 12 && &h[4..8] == b"ftyp" {
        let size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]);
        if (8..=512).contains(&size) {
            return Some(Family::IsoBmff);
        }
    }
    None
}

/// Determina lunghezza e tipo del file che inizia a `start`.
pub fn measure(r: &mut ByteReader, family: Family, start: u64) -> Option<(u64, FileKind)> {
    let max = family.max_size();
    match family {
        Family::Jpeg => jpeg(r, start, max).map(|n| (n, FileKind::Jpeg)),
        Family::Png => png(r, start, max).map(|n| (n, FileKind::Png)),
        Family::Gif => gif(r, start, max).map(|n| (n, FileKind::Gif)),
        Family::Pdf => pdf(r, start, max).map(|n| (n, FileKind::Pdf)),
        Family::Zip => zip(r, start, max),
        Family::IsoBmff => iso_bmff(r, start, max),
    }
}

/// JPEG: cammina i marker fino a EOI, saltando i dati entropici dopo SOS.
fn jpeg(r: &mut ByteReader, start: u64, max: u64) -> Option<u64> {
    let mut pos = start + 2;
    let mut seen_sos = false;
    loop {
        if pos - start > max {
            return None;
        }
        if r.byte(pos)? != 0xFF {
            return None;
        }
        let mut marker = r.byte(pos + 1)?;
        while marker == 0xFF {
            pos += 1;
            marker = r.byte(pos + 1)?;
        }
        match marker {
            0xD9 => {
                return if seen_sos {
                    Some(pos + 2 - start)
                } else {
                    None
                };
            }
            0xD8 | 0x00 => return None,
            0x01 | 0xD0..=0xD7 => pos += 2,
            0xDA => {
                let len = r.u16_be(pos + 2)? as u64;
                if len < 2 {
                    return None;
                }
                pos += 2 + len;
                seen_sos = true;
                // Dati entropici: 0xFF00 è un byte di dati, 0xFFD0..D7 sono restart.
                loop {
                    if pos - start > max {
                        return None;
                    }
                    if r.byte(pos)? != 0xFF {
                        pos += 1;
                        continue;
                    }
                    let next = r.byte(pos + 1)?;
                    match next {
                        0x00 | 0xD0..=0xD7 => pos += 2,
                        0xFF => pos += 1,
                        _ => break,
                    }
                }
            }
            _ => {
                let len = r.u16_be(pos + 2)? as u64;
                if len < 2 {
                    return None;
                }
                pos += 2 + len;
            }
        }
    }
}

/// PNG: cammina i chunk fino a IEND. Il primo chunk deve essere IHDR.
fn png(r: &mut ByteReader, start: u64, max: u64) -> Option<u64> {
    let mut pos = start + 8;
    let mut first = true;
    loop {
        let len = r.u32_be(pos)? as u64;
        if len > 0x7FFF_FFFF {
            return None;
        }
        let kind: [u8; 4] = r.array(pos + 4)?;
        if !kind.iter().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        if first && &kind != b"IHDR" {
            return None;
        }
        first = false;
        pos += 12 + len;
        if pos - start > max {
            return None;
        }
        if &kind == b"IEND" {
            return Some(pos - start);
        }
    }
}

/// Salta una sequenza di sub-block GIF (lunghezza + dati, terminata da 0).
fn gif_sub_blocks(r: &mut ByteReader, mut pos: u64) -> Option<u64> {
    loop {
        let n = r.byte(pos)? as u64;
        pos += 1;
        if n == 0 {
            return Some(pos);
        }
        pos += n;
    }
}

/// GIF: header, palette globale, poi blocchi immagine/estensione fino al trailer 0x3B.
fn gif(r: &mut ByteReader, start: u64, max: u64) -> Option<u64> {
    let flags = r.byte(start + 10)?;
    let mut pos = start + 13;
    if flags & 0x80 != 0 {
        pos += 3 * (1u64 << ((flags & 0x07) + 1));
    }
    loop {
        if pos - start > max {
            return None;
        }
        match r.byte(pos)? {
            0x3B => return Some(pos + 1 - start),
            0x21 => pos = gif_sub_blocks(r, pos + 2)?,
            0x2C => {
                let lflags = r.byte(pos + 9)?;
                pos += 10;
                if lflags & 0x80 != 0 {
                    pos += 3 * (1u64 << ((lflags & 0x07) + 1));
                }
                pos += 1; // LZW minimum code size
                pos = gif_sub_blocks(r, pos)?;
            }
            _ => return None,
        }
    }
}

/// PDF: fino al primo `%%EOF` (più il fine riga).
/// Limite noto: i PDF con aggiornamenti incrementali hanno più `%%EOF`;
/// qui si recupera solo la prima revisione.
fn pdf(r: &mut ByteReader, start: u64, max: u64) -> Option<u64> {
    let eof = r.find(start + 8, start + max, b"%%EOF")?;
    let mut end = eof + 5;
    if r.byte(end) == Some(b'\r') {
        end += 1;
    }
    if r.byte(end) == Some(b'\n') {
        end += 1;
    }
    Some(end - start)
}

/// ZIP e formati Office: cerca l'End Of Central Directory coerente con
/// l'inizio dell'archivio, poi classifica dai nomi nella central directory.
fn zip(r: &mut ByteReader, start: u64, max: u64) -> Option<(u64, FileKind)> {
    let mut from = start + 30;
    loop {
        let eocd = r.find(from, start + max, b"PK\x05\x06")?;
        let cd_size = r.u32_le(eocd + 12)? as u64;
        let cd_offset = r.u32_le(eocd + 16)? as u64;
        let comment = r.u16_le(eocd + 20)? as u64;
        if start + cd_offset + cd_size == eocd {
            let kind = classify_zip(r, start + cd_offset, eocd);
            return Some((eocd + 22 + comment - start, kind));
        }
        // EOCD di un archivio annidato o un falso positivo: continua.
        from = eocd + 4;
    }
}

fn classify_zip(r: &mut ByteReader, mut pos: u64, end: u64) -> FileKind {
    while pos + 46 <= end {
        if r.array::<4>(pos) != Some(*b"PK\x01\x02") {
            break;
        }
        let (Some(name_len), Some(extra), Some(comment)) =
            (r.u16_le(pos + 28), r.u16_le(pos + 30), r.u16_le(pos + 32))
        else {
            break;
        };
        let mut name = vec![0u8; name_len as usize];
        if r.read(pos + 46, &mut name).is_none() {
            break;
        }
        if name.starts_with(b"word/") {
            return FileKind::Docx;
        }
        if name.starts_with(b"xl/") {
            return FileKind::Xlsx;
        }
        if name.starts_with(b"ppt/") {
            return FileKind::Pptx;
        }
        pos += 46 + name_len as u64 + extra as u64 + comment as u64;
    }
    FileKind::Zip
}

/// ISO Base Media (MP4, MOV, HEIC): cammina i box di primo livello.
fn iso_bmff(r: &mut ByteReader, start: u64, max: u64) -> Option<(u64, FileKind)> {
    const TOP_LEVEL: [&[u8; 4]; 14] = [
        b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"uuid", b"meta", b"pdin", b"moof",
        b"mfra", b"styp", b"sidx", b"pnot",
    ];
    let brand: [u8; 4] = r.array(start + 8)?;
    let kind = match &brand {
        b"qt  " => FileKind::Mov,
        b"heic" | b"heix" | b"mif1" | b"msf1" | b"hevc" | b"heim" | b"heis" => FileKind::Heic,
        _ => FileKind::Mp4,
    };

    let mut pos = start;
    let mut has_payload = false;
    let mut has_index = false;
    loop {
        if pos - start > max {
            return None;
        }
        let Some(size32) = r.u32_be(pos) else { break };
        let Some(typ) = r.array::<4>(pos + 4) else {
            break;
        };
        if !TOP_LEVEL.contains(&&typ) {
            break;
        }
        let size = match size32 {
            0 => return None, // "fino alla fine del file": non determinabile
            1 => r.u64_be(pos + 8)?,
            n => n as u64,
        };
        if size < 8 {
            break;
        }
        match &typ {
            b"mdat" | b"moof" => has_payload = true,
            b"moov" => has_index = true,
            b"meta" => {
                has_index = true;
                has_payload = true;
            }
            _ => {}
        }
        pos += size;
        if pos > r.size() {
            return None;
        }
    }
    if pos == start || !has_index || !has_payload {
        return None;
    }
    Some((pos - start, kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemorySource;

    fn measure_bytes(data: &[u8]) -> Option<(u64, FileKind)> {
        let fam = detect(data)?;
        let src = MemorySource::new(data.to_vec(), "t");
        let mut r = ByteReader::new(&src);
        measure(&mut r, fam, 0)
    }

    #[test]
    fn truncated_png_is_rejected() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0u8; 17]);
        // manca IEND
        assert_eq!(measure_bytes(&png), None);
    }

    #[test]
    fn jpeg_without_scan_is_rejected() {
        // SOI + APP0 vuoto + EOI senza SOS: non è un'immagine.
        let data = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02, 0xFF, 0xD9];
        assert_eq!(measure_bytes(&data), None);
    }

    #[test]
    fn mp4_needs_moov_and_mdat() {
        let mut v = Vec::new();
        v.extend_from_slice(&16u32.to_be_bytes());
        v.extend_from_slice(b"ftypisom\0\0\0\0");
        v.extend_from_slice(&12u32.to_be_bytes());
        v.extend_from_slice(b"mdat1234");
        assert_eq!(
            measure_bytes(&v),
            None,
            "senza moov non è un video riproducibile"
        );
    }

    #[test]
    fn extensions_roundtrip() {
        for k in FileKind::ALL {
            assert_eq!(FileKind::from_extension(k.extension()), Some(k));
        }
        assert_eq!(FileKind::from_extension(".JPEG"), Some(FileKind::Jpeg));
    }
}
