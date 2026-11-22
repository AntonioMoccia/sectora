//! Test di riferimento: si costruisce un'immagine disco piena di rumore,
//! si scrivono file reali a offset noti, e il Deep Scan deve ritrovarli
//! tutti con hash SHA-256 identico e nessun falso positivo.

use sectora_engine::{carve, recover_file, CarveOptions, FileKind, Flow, ReadOnlySource};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};

/// Rumore deterministico (xorshift), così il test è ripetibile.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

fn gradient(w: u32, h: u32) -> image::RgbImage {
    image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([
            (x * 7 % 256) as u8,
            (y * 5 % 256) as u8,
            ((x + y) % 256) as u8,
        ])
    })
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&gradient(w, h))
        .unwrap();
    out
}

/// JPEG con una miniatura JPEG dentro un segmento APP1, come le foto EXIF:
/// il primo FFD9 è quello della miniatura, non della foto.
fn jpeg_with_thumbnail() -> Vec<u8> {
    let main = jpeg(320, 240);
    let thumb = jpeg(16, 12);
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&thumb);
    let mut out = main[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&main[2..]);
    out
}

fn png() -> Vec<u8> {
    let mut out = Vec::new();
    gradient(97, 61)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn gif() -> Vec<u8> {
    let mut out = Vec::new();
    let img = image::DynamicImage::ImageRgb8(gradient(40, 30)).to_rgba8();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
        enc.encode_frame(image::Frame::new(img)).unwrap();
    }
    out
}

fn pdf() -> Vec<u8> {
    let body = "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n\
trailer << /Root 1 0 R >>\n%%EOF\n";
    body.as_bytes().to_vec()
}

fn office_zip(entries: &[(&str, &str)], inner_zip: Option<&[u8]>) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            z.start_file(*name, opts).unwrap();
            z.write_all(content.as_bytes()).unwrap();
        }
        if let Some(inner) = inner_zip {
            let stored = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            z.start_file("allegato.zip", stored).unwrap();
            z.write_all(inner).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

fn mp4(brand: &[u8; 4]) -> Vec<u8> {
    let mut v = Vec::new();
    let ftyp_len = 20u32;
    v.extend_from_slice(&ftyp_len.to_be_bytes());
    v.extend_from_slice(b"ftyp");
    v.extend_from_slice(brand);
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(brand);
    let media = noise(70_000, 99);
    v.extend_from_slice(&((media.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(b"mdat");
    v.extend_from_slice(&media);
    let mvhd = vec![0u8; 100];
    v.extend_from_slice(&((mvhd.len() + 16) as u32).to_be_bytes());
    v.extend_from_slice(b"moov");
    v.extend_from_slice(&((mvhd.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(b"mvhd");
    v.extend_from_slice(&mvhd);
    v
}

fn sha(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

#[test]
fn deep_scan_finds_every_file_with_identical_hash() {
    let inner_zip = office_zip(&[("leggimi.txt", "zip dentro un docx")], None);
    let files: Vec<(u64, FileKind, Vec<u8>)> = vec![
        (4096, FileKind::Jpeg, jpeg_with_thumbnail()),
        (200 * 512, FileKind::Png, png()),
        (400 * 512, FileKind::Gif, gif()),
        (600 * 512, FileKind::Pdf, pdf()),
        (
            800 * 512,
            FileKind::Docx,
            office_zip(
                &[
                    ("[Content_Types].xml", "<Types/>"),
                    ("word/document.xml", "<w:document>ciao</w:document>"),
                ],
                Some(&inner_zip),
            ),
        ),
        (
            1000 * 512,
            FileKind::Xlsx,
            office_zip(&[("xl/workbook.xml", "<workbook/>")], None),
        ),
        (1200 * 512, FileKind::Zip, inner_zip.clone()),
        (1400 * 512, FileKind::Mp4, mp4(b"isom")),
        (1800 * 512, FileKind::Mov, mp4(b"qt  ")),
        (2200 * 512, FileKind::Jpeg, jpeg(64, 48)),
    ];

    let mut disk = noise(2400 * 512, 42);
    for (off, _, data) in &files {
        let off = *off as usize;
        disk[off..off + data.len()].copy_from_slice(data);
    }
    // Verifica di coerenza del test: i file non si sovrappongono.
    for w in files.windows(2) {
        assert!(
            w[0].0 + w[0].2.len() as u64 <= w[1].0,
            "file sovrapposti nel test"
        );
    }

    let dir = tempfile::tempdir().unwrap();
    let img_path = dir.path().join("disk.img");
    std::fs::write(&img_path, &disk).unwrap();
    let src = ReadOnlySource::open(&img_path).unwrap();

    let found = carve(&src, &CarveOptions::default(), |_| Flow::Continue);

    let got: Vec<(u64, FileKind, u64)> =
        found.iter().map(|f| (f.offset, f.kind, f.length)).collect();
    let want: Vec<(u64, FileKind, u64)> = files
        .iter()
        .map(|(o, k, d)| (*o, *k, d.len() as u64))
        .collect();
    assert_eq!(got, want);

    let out = dir.path().join("recuperati");
    for (f, (_, _, original)) in found.iter().zip(&files) {
        let path = recover_file(&src, f, &out).unwrap();
        let recovered = std::fs::read(&path).unwrap();
        assert_eq!(sha(&recovered), sha(original), "hash diverso per {path:?}");
    }
}

#[test]
fn type_filter_and_stop() {
    let mut disk = noise(64 * 512, 7);
    let a = jpeg(32, 32);
    let b = pdf();
    disk[512..512 + a.len()].copy_from_slice(&a);
    disk[40 * 512..40 * 512 + b.len()].copy_from_slice(&b);
    let src = sectora_engine::MemorySource::new(disk, "mem");

    let opts = CarveOptions {
        kinds: [FileKind::Pdf].into_iter().collect(),
        ..Default::default()
    };
    let found = carve(&src, &opts, |_| Flow::Continue);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, FileKind::Pdf);

    let found = carve(&src, &CarveOptions::default(), |e| match e {
        sectora_engine::Event::Found(_) => Flow::Stop,
        _ => Flow::Continue,
    });
    assert_eq!(found.len(), 1, "si ferma al primo file trovato");
}

/// Sorgente con un intervallo di settori illeggibili.
struct BadSectors {
    inner: sectora_engine::MemorySource,
    bad: std::ops::Range<u64>,
}

impl sectora_engine::BlockSource for BadSectors {
    fn size(&self) -> u64 {
        self.inner.size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        if offset < self.bad.end && offset + buf.len() as u64 > self.bad.start {
            return Err(std::io::Error::other("settore illeggibile"));
        }
        self.inner.read_at(offset, buf)
    }
    fn describe(&self) -> String {
        "bad".into()
    }
}

#[test]
fn bad_sector_does_not_hide_files_in_same_chunk() {
    // Un solo blocco da 4 MB: settori danneggiati all'inizio, una foto più avanti.
    let mut disk = noise(4 * 1024 * 1024, 11);
    let photo = jpeg(120, 90);
    let at = 2 * 1024 * 1024;
    disk[at..at + photo.len()].copy_from_slice(&photo);
    let src = BadSectors {
        inner: sectora_engine::MemorySource::new(disk, "mem"),
        bad: 8192..8192 + 1024,
    };
    let mut errors = Vec::new();
    let found = carve(&src, &CarveOptions::default(), |e| {
        if let sectora_engine::Event::ReadError { offset, length, .. } = e {
            errors.push((*offset, *length));
        }
        Flow::Continue
    });
    assert_eq!(
        errors,
        vec![(8192, 1024)],
        "solo i 2 settori danneggiati sono segnalati"
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].offset, at as u64);
    assert_eq!(found[0].length, photo.len() as u64);
}
