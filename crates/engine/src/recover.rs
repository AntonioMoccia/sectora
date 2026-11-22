//! Copia dei file trovati e creazione di immagini disco.
//!
//! Tutte le scritture avvengono su una destinazione che deve essere diversa
//! dalla sorgente: scrivere sul disco che si sta recuperando sovrascrive
//! proprio i dati che si vogliono salvare.

use crate::carve::Found;
use crate::source::{BlockSource, ReadOnlySource};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum RecoverError {
    #[error("la destinazione {0} si trova sullo stesso disco fisico della sorgente: scegli un altro disco")]
    SameDevice(PathBuf),
    #[error("la destinazione {0} è la sorgente stessa")]
    SameFile(PathBuf),
    #[error("impossibile verificare che {0} sia su un disco diverso dalla sorgente ({1}): per sicurezza il recupero è bloccato")]
    Unverifiable(PathBuf, String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Verifica che `dest_dir` non stia sullo stesso disco fisico della sorgente.
///
/// Sorgente dispositivo: sorgente e destinazione vengono risolte fino ai dischi
/// fisici (partizioni, volumi APFS, LVM/device-mapper); se hanno un disco in
/// comune il recupero viene rifiutato. Se la verifica non è possibile il
/// recupero viene bloccato: scrivere sul disco sorgente distrugge i dati.
/// Sorgente immagine: rifiuta solo se la destinazione è il file stesso.
pub fn check_destination(source: &ReadOnlySource, dest_dir: &Path) -> Result<(), RecoverError> {
    // Nessuna scrittura prima della verifica: si controlla la cartella esistente più vicina,
    // e la destinazione viene creata solo se il controllo passa.
    let mut existing = if dest_dir.is_absolute() {
        dest_dir.to_path_buf()
    } else {
        std::env::current_dir()?.join(dest_dir)
    };
    while !existing.exists() {
        if !existing.pop() {
            return Err(RecoverError::Unverifiable(
                dest_dir.to_path_buf(),
                "percorso non valido".into(),
            ));
        }
    }
    let dest = fs::canonicalize(&existing)?;
    if let Ok(src) = fs::canonicalize(source.path()) {
        if dest == src || dest.starts_with(&src) {
            return Err(RecoverError::SameFile(dest));
        }
    }
    if !source.is_device() {
        fs::create_dir_all(dest_dir)?;
        return Ok(());
    }
    let src_disks = platform::physical_disks_of_device(source.path())
        .map_err(|e| RecoverError::Unverifiable(dest.clone(), e))?;
    let dest_disks = platform::physical_disks_of_path(&dest)
        .map_err(|e| RecoverError::Unverifiable(dest.clone(), e))?;
    if src_disks.iter().any(|d| dest_disks.contains(d)) {
        return Err(RecoverError::SameDevice(dest));
    }
    fs::create_dir_all(dest_dir)?;
    Ok(())
}

#[cfg(target_os = "linux")]
mod platform {
    use std::collections::BTreeSet;
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    fn major_minor(dev: u64) -> (u64, u64) {
        // Codifica glibc di dev_t.
        let major = ((dev >> 32) & 0xFFFF_F000) | ((dev >> 8) & 0x0000_0FFF);
        let minor = ((dev >> 12) & 0xFFFF_FF00) | (dev & 0x0000_00FF);
        (major, minor)
    }

    /// Dischi fisici sotto un dispositivo a blocchi: risale da partizione a disco
    /// e segue gli "slaves" di device-mapper (LVM, LUKS).
    fn disks_of_devnum(dev: u64, out: &mut BTreeSet<String>, depth: u32) -> Result<(), String> {
        let (maj, min) = major_minor(dev);
        let link = PathBuf::from(format!("/sys/dev/block/{maj}:{min}"));
        let real = fs::canonicalize(&link).map_err(|e| format!("{}: {e}", link.display()))?;
        let mut sysdir = real.clone();
        // Partizione: la directory del disco è la madre.
        if real.join("partition").exists() {
            if let Some(p) = real.parent() {
                sysdir = p.to_path_buf();
            }
        }
        let slaves = sysdir.join("slaves");
        let mut had_slave = false;
        if depth < 8 {
            if let Ok(entries) = fs::read_dir(&slaves) {
                for e in entries.flatten() {
                    if let Ok(devstr) = fs::read_to_string(e.path().join("dev")) {
                        if let Some((a, b)) = devstr.trim().split_once(':') {
                            let (a, b): (u64, u64) =
                                (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
                            let num = ((a & 0xFFF) << 8)
                                | (b & 0xFF)
                                | ((b & !0xFF) << 12)
                                | ((a & !0xFFF) << 32);
                            had_slave = true;
                            disks_of_devnum(num, out, depth + 1)?;
                        }
                    }
                }
            }
        }
        if !had_slave {
            let name = sysdir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.insert(name);
        }
        Ok(())
    }

    pub fn physical_disks_of_device(device: &Path) -> Result<BTreeSet<String>, String> {
        let rdev = fs::metadata(device).map_err(|e| e.to_string())?.rdev();
        let mut out = BTreeSet::new();
        disks_of_devnum(rdev, &mut out, 0)?;
        Ok(out)
    }

    pub fn physical_disks_of_path(path: &Path) -> Result<BTreeSet<String>, String> {
        let dev = fs::metadata(path).map_err(|e| e.to_string())?.dev();
        let (maj, _) = major_minor(dev);
        let mut out = BTreeSet::new();
        // Major 0 = file system virtuali (tmpfs, overlay, FUSE, rete): non stanno su un disco a blocchi.
        if maj == 0 {
            return Ok(out);
        }
        disks_of_devnum(dev, &mut out, 0)?;
        Ok(out)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::collections::BTreeSet;
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::process::Command;

    /// `disk3s5` → `disk3`.
    fn whole_disk(name: &str) -> Option<String> {
        let rest = name.strip_prefix("disk")?;
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        (!digits.is_empty()).then(|| format!("disk{digits}"))
    }

    /// Dischi fisici di un disco: per i volumi APFS il disco "sintetizzato"
    /// (es. disk3) sta sopra uno store fisico (es. disk0s2), letto da `diskutil`.
    fn physical(disk: &str) -> Result<BTreeSet<String>, String> {
        let mut out = BTreeSet::new();
        out.insert(disk.to_string());
        let output = Command::new("/usr/sbin/diskutil")
            .args(["info", "-plist", disk])
            .output()
            .map_err(|e| format!("diskutil: {e}"))?;
        if !output.status.success() {
            return Err(format!("diskutil info {disk} non riuscito"));
        }
        let plist = String::from_utf8_lossy(&output.stdout);
        if let Some(i) = plist.find("<key>APFSPhysicalStores</key>") {
            let tail = &plist[i..];
            let end = tail.find("</array>").unwrap_or(tail.len());
            for part in tail[..end].split("<string>").skip(1) {
                if let Some(id) = part.split("</string>").next() {
                    if let Some(w) = whole_disk(id.trim()) {
                        out.insert(w);
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn physical_disks_of_device(device: &Path) -> Result<BTreeSet<String>, String> {
        let name = device
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = name.strip_prefix('r').unwrap_or(&name);
        let disk = whole_disk(name)
            .ok_or_else(|| format!("dispositivo non riconosciuto: {}", device.display()))?;
        physical(&disk)
    }

    pub fn physical_disks_of_path(path: &Path) -> Result<BTreeSet<String>, String> {
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: statfs riempie la struttura passata; il percorso è una stringa C valida.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY: f_mntfromname è una stringa C terminata da NUL dentro un array fisso.
        let from = unsafe { CStr::from_ptr(st.f_mntfromname.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        match from.strip_prefix("/dev/").and_then(whole_disk) {
            Some(disk) => physical(&disk),
            // Volume di rete o virtuale: non sta su un disco locale.
            None => Ok(BTreeSet::new()),
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::BTreeSet;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetVolumePathNameW, FILE_SHARE_READ, FILE_SHARE_WRITE,
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Ioctl::{DISK_EXTENT, VOLUME_DISK_EXTENTS};
    use windows_sys::Win32::System::IO::DeviceIoControl;

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(Some(0))
            .collect()
    }

    /// Numeri dei dischi fisici su cui sta il volume `\\.\C:`.
    fn volume_disks(volume_device: &str) -> Result<BTreeSet<String>, String> {
        let name = wide(volume_device);
        // SAFETY: chiamate Win32 con buffer della dimensione dichiarata; l'handle viene chiuso.
        unsafe {
            let h = CreateFileW(
                name.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                return Err(format!(
                    "{volume_device}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // Spazio per fino a 32 extent (volumi su più dischi).
            let mut buf = vec![
                0u8;
                std::mem::size_of::<VOLUME_DISK_EXTENTS>()
                    + 32 * std::mem::size_of::<DISK_EXTENT>()
            ];
            let mut returned = 0u32;
            let ok = DeviceIoControl(
                h,
                IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                std::ptr::null(),
                0,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            );
            CloseHandle(h);
            if ok == 0 {
                return Err(format!(
                    "{volume_device}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
            // Gli extent iniziano dopo NumberOfDiskExtents, allineati a 8 byte.
            let base = std::mem::offset_of!(VOLUME_DISK_EXTENTS, Extents);
            let size = std::mem::size_of::<DISK_EXTENT>();
            let mut out = BTreeSet::new();
            for i in 0..count.min(32) {
                let o = base + i * size;
                let disk = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
                out.insert(format!("PhysicalDrive{disk}"));
            }
            Ok(out)
        }
    }

    pub fn physical_disks_of_device(device: &Path) -> Result<BTreeSet<String>, String> {
        let s = device.to_string_lossy();
        let lower = s.to_ascii_lowercase();
        if let Some(n) = lower.strip_prefix(r"\\.\physicaldrive") {
            let n: u32 = n
                .parse()
                .map_err(|_| format!("dispositivo non riconosciuto: {s}"))?;
            return Ok([format!("PhysicalDrive{n}")].into_iter().collect());
        }
        // Volume, es. \\.\E:
        volume_disks(&s)
    }

    pub fn physical_disks_of_path(path: &Path) -> Result<BTreeSet<String>, String> {
        let p: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut vol = vec![0u16; 1024];
        // SAFETY: buffer di 1024 caratteri dichiarato alla funzione.
        let ok = unsafe { GetVolumePathNameW(p.as_ptr(), vol.as_mut_ptr(), vol.len() as u32) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let len = vol.iter().position(|c| *c == 0).unwrap_or(vol.len());
        let root = String::from_utf16_lossy(&vol[..len]);
        // `\\?\` (percorso canonico) o UNC di rete.
        let root = root.strip_prefix(r"\\?\").unwrap_or(&root).to_string();
        if root.starts_with(r"\\") || root.to_ascii_uppercase().starts_with("UNC") {
            return Ok(BTreeSet::new());
        }
        let letter = root.trim_end_matches('\\');
        volume_disks(&format!(r"\\.\{letter}"))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use std::collections::BTreeSet;
    use std::path::Path;

    pub fn physical_disks_of_device(_device: &Path) -> Result<BTreeSet<String>, String> {
        Err("sistema operativo non supportato per la verifica".into())
    }
    pub fn physical_disks_of_path(_path: &Path) -> Result<BTreeSet<String>, String> {
        Err("sistema operativo non supportato per la verifica".into())
    }
}

/// Nome del file recuperato: `<categoria>/<tipo>/f<offset>.<ext>`.
pub fn output_path(dest_dir: &Path, found: &Found) -> PathBuf {
    dest_dir
        .join(found.kind.category())
        .join(found.kind.extension())
        .join(format!("f{:014}.{}", found.offset, found.kind.extension()))
}

/// Copia un file trovato nella destinazione. Non sovrascrive mai file esistenti.
pub fn recover_file(src: &dyn BlockSource, found: &Found, dest_dir: &Path) -> io::Result<PathBuf> {
    let path = output_path(dest_dir, found);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    copy_range(src, found.offset, found.length, &mut out)?;
    out.sync_all()?;
    Ok(path)
}

fn copy_range(src: &dyn BlockSource, offset: u64, length: u64, out: &mut File) -> io::Result<()> {
    let mut buf = vec![0u8; 1024 * 1024];
    let mut done = 0u64;
    while done < length {
        let want = (length - done).min(buf.len() as u64) as usize;
        crate::source::read_exact_at(src, offset + done, &mut buf[..want])?;
        out.write_all(&buf[..want])?;
        done += want as u64;
    }
    Ok(())
}

/// Recupera un file trovato dal Quick Scan mantenendo il percorso originale:
/// `<dest>/<volume>/<cartelle>/<nome>`. Se il nome esiste già aggiunge " (n)".
pub fn recover_entry(
    src: &dyn BlockSource,
    vol: &crate::fs::Volume,
    entry: &crate::fs::FileEntry,
    dest_dir: &Path,
) -> io::Result<PathBuf> {
    use crate::fs::sanitize_component;
    let mut path = dest_dir.join(sanitize_component(&vol.display_name()));
    let parts: Vec<&str> = entry.path.split('/').filter(|p| !p.is_empty()).collect();
    for p in &parts {
        path.push(sanitize_component(p));
    }
    if entry.is_dir {
        fs::create_dir_all(&path)?;
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut candidate = path.clone();
    let mut n = 1;
    let mut out = loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => break f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < 1000 => {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let ext = path
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default();
                candidate = path.with_file_name(format!("{stem} ({n}){ext}"));
                n += 1;
            }
            Err(e) => return Err(e),
        }
    };
    crate::fs::copy_entry(src, vol, entry, &mut out)?;
    // Data di modifica originale. Solo NTFS la salva in UTC; FAT ed exFAT usano
    // l'ora locale del dispositivo, che non è nota con certezza.
    if vol.fs == Some(crate::fs::FsType::Ntfs) {
        if let Some(ts) = entry.modified.filter(|t| *t > 0) {
            let _ =
                out.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(ts as u64));
        }
    }
    out.sync_all()?;
    Ok(candidate)
}

/// Esito della creazione di un'immagine disco.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ImageReport {
    pub bytes: u64,
    /// Intervalli illeggibili, riempiti di zeri nell'immagine: (offset, lunghezza).
    pub bad_ranges: Vec<(u64, u64)>,
}

/// Crea un'immagine bit per bit della sorgente. Le aree illeggibili vengono
/// rilette settore per settore; quelle che falliscono ancora diventano zeri
/// e sono elencate nel report.
pub fn create_image(
    src: &dyn BlockSource,
    out_path: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> io::Result<ImageReport> {
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out_path)?;
    let total = src.size();
    let sector = src.sector_size() as usize;
    let chunk = 1024 * 1024usize;
    let mut buf = vec![0u8; chunk];
    let mut report = ImageReport::default();
    let mut pos = 0u64;

    while pos < total {
        let want = chunk.min((total - pos) as usize);
        match crate::source::read_exact_at(src, pos, &mut buf[..want]) {
            Ok(()) => out.write_all(&buf[..want])?,
            Err(_) => {
                // Rilettura settore per settore.
                let mut off = 0;
                while off < want {
                    let n = sector.min(want - off);
                    let piece = &mut buf[off..off + n];
                    if crate::source::read_exact_at(src, pos + off as u64, piece).is_err() {
                        piece.fill(0);
                        push_bad(&mut report.bad_ranges, pos + off as u64, n as u64);
                    }
                    off += n;
                }
                out.write_all(&buf[..want])?;
            }
        }
        pos += want as u64;
        report.bytes = pos;
        on_progress(pos, total);
    }
    out.sync_all()?;
    Ok(report)
}

fn push_bad(ranges: &mut Vec<(u64, u64)>, offset: u64, len: u64) {
    if let Some(last) = ranges.last_mut() {
        if last.0 + last.1 == offset {
            last.1 += len;
            return;
        }
    }
    ranges.push((offset, len));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carve::FileKind;
    use crate::source::MemorySource;

    struct Flaky {
        inner: MemorySource,
        bad: std::ops::Range<u64>,
    }

    impl BlockSource for Flaky {
        fn size(&self) -> u64 {
            self.inner.size()
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            let end = offset + buf.len() as u64;
            if offset < self.bad.end && end > self.bad.start {
                return Err(io::Error::other("settore illeggibile"));
            }
            self.inner.read_at(offset, buf)
        }
        fn describe(&self) -> String {
            "flaky".into()
        }
    }

    #[test]
    fn image_zero_fills_bad_sectors() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8 + 1).collect();
        let src = Flaky {
            inner: MemorySource::new(data.clone(), "mem"),
            bad: 1024..1536,
        };
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("disk.img");
        let report = create_image(&src, &out, |_, _| {}).unwrap();
        assert_eq!(report.bytes, 4096);
        assert_eq!(report.bad_ranges, vec![(1024, 512)]);
        let img = fs::read(&out).unwrap();
        assert_eq!(&img[..1024], &data[..1024]);
        assert!(img[1024..1536].iter().all(|b| *b == 0));
        assert_eq!(&img[1536..], &data[1536..]);
    }

    #[test]
    fn never_overwrites() {
        let src = MemorySource::new(vec![7u8; 1024], "mem");
        let dir = tempfile::tempdir().unwrap();
        let f = Found {
            kind: FileKind::Pdf,
            offset: 0,
            length: 10,
        };
        recover_file(&src, &f, dir.path()).unwrap();
        assert!(recover_file(&src, &f, dir.path()).is_err());
    }

    #[test]
    fn rejects_destination_inside_image_path() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("disk.img");
        fs::write(&img, [0u8; 512]).unwrap();
        let src = ReadOnlySource::open(&img).unwrap();
        // Una cartella accanto all'immagine va bene.
        assert!(check_destination(&src, &dir.path().join("out")).is_ok());
    }
}
