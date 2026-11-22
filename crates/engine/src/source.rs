//! Accesso in sola lettura a dischi, partizioni e immagini disco.
//!
//! Regola di sicurezza del progetto: questo è l'unico modulo che apre la
//! sorgente, e non espone nessuna API di scrittura. Il trait [`BlockSource`]
//! ha solo metodi di lettura, e i file vengono aperti con `read(true)` e
//! nient'altro.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Una sorgente di byte leggibile ad accesso casuale e mai scrivibile.
pub trait BlockSource: Send + Sync {
    /// Dimensione totale in byte.
    fn size(&self) -> u64;

    /// Legge fino a `buf.len()` byte a partire da `offset`.
    /// Ritorna il numero di byte letti (0 = fine della sorgente).
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    /// Dimensione del settore logico. Gli offset dei file recuperabili sono
    /// quasi sempre allineati a questo valore.
    fn sector_size(&self) -> u32 {
        512
    }

    /// Descrizione leggibile (percorso o nome del dispositivo).
    fn describe(&self) -> String;
}

/// Legge esattamente `buf.len()` byte, o fallisce.
pub fn read_exact_at(src: &dyn BlockSource, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let n = src.read_at(offset, buf)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "lettura oltre la fine della sorgente",
            ));
        }
        offset += n as u64;
        buf = &mut buf[n..];
    }
    Ok(())
}

/// Un file immagine o un dispositivo a blocchi aperto in sola lettura.
pub struct ReadOnlySource {
    file: File,
    path: PathBuf,
    size: u64,
    sector: u32,
    /// I dispositivi raw (macOS `/dev/rdiskN`, Windows `\\.\PhysicalDriveN`)
    /// accettano solo letture allineate al settore.
    aligned: bool,
}

impl ReadOnlySource {
    /// Apre un'immagine disco o un dispositivo in sola lettura.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().read(true).open(&path)?;
        let is_regular = file.metadata().map(|m| m.is_file()).unwrap_or(false);

        let (size, sector, aligned) = if is_regular {
            (file.metadata()?.len(), 512, false)
        } else {
            let (size, sector) = platform::device_geometry(&file)?;
            (size, sector.max(512), true)
        };

        Ok(Self {
            file,
            path,
            size,
            sector,
            aligned,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `true` se la sorgente è un dispositivo e non un file immagine.
    pub fn is_device(&self) -> bool {
        self.aligned
    }

    fn read_aligned(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let sector = self.sector as u64;
        let start = offset - offset % sector;
        let end = (offset + buf.len() as u64).min(self.size);
        if end <= offset {
            return Ok(0);
        }
        let end_aligned = end.div_ceil(sector) * sector;
        let mut tmp = vec![0u8; (end_aligned - start) as usize];
        let mut filled = 0usize;
        while filled < tmp.len() {
            let n = platform::pread(&self.file, &mut tmp[filled..], start + filled as u64)?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        let skip = (offset - start) as usize;
        if filled <= skip {
            return Ok(0);
        }
        let n = (filled - skip).min(buf.len()).min((end - offset) as usize);
        buf[..n].copy_from_slice(&tmp[skip..skip + n]);
        Ok(n)
    }
}

impl BlockSource for ReadOnlySource {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let max = ((self.size - offset).min(buf.len() as u64)) as usize;
        let buf = &mut buf[..max];
        if self.aligned {
            self.read_aligned(offset, buf)
        } else {
            platform::pread(&self.file, buf, offset)
        }
    }

    fn sector_size(&self) -> u32 {
        self.sector
    }

    fn describe(&self) -> String {
        self.path.display().to_string()
    }
}

/// Sorgente in memoria, utile nei test e per l'anteprima.
pub struct MemorySource {
    data: Vec<u8>,
    name: String,
}

impl MemorySource {
    pub fn new(data: Vec<u8>, name: impl Into<String>) -> Self {
        Self {
            data,
            name: name.into(),
        }
    }
}

impl BlockSource for MemorySource {
    fn size(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.data.len() as u64;
        if offset >= len {
            return Ok(0);
        }
        let start = offset as usize;
        let n = buf.len().min(self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        Ok(n)
    }

    fn describe(&self) -> String {
        self.name.clone()
    }
}

#[cfg(unix)]
mod platform {
    use std::fs::File;
    use std::io;
    use std::os::unix::fs::FileExt;
    use std::os::unix::io::AsRawFd;

    pub fn pread(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        loop {
            match file.read_at(buf, offset) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                other => return other,
            }
        }
    }

    /// Dimensione e settore di un dispositivo a blocchi.
    #[cfg(target_os = "macos")]
    pub fn device_geometry(file: &File) -> io::Result<(u64, u32)> {
        // Valori da <sys/disk.h>: _IOR('d', 24, uint32_t) e _IOR('d', 25, uint64_t)
        const DKIOCGETBLOCKSIZE: libc::c_ulong = 0x4004_6418;
        const DKIOCGETBLOCKCOUNT: libc::c_ulong = 0x4008_6419;
        let fd = file.as_raw_fd();
        let mut block_size: u32 = 0;
        let mut block_count: u64 = 0;
        // SAFETY: ioctl in sola lettura su un fd valido, con buffer della dimensione attesa.
        unsafe {
            if libc::ioctl(fd, DKIOCGETBLOCKSIZE, &mut block_size) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(fd, DKIOCGETBLOCKCOUNT, &mut block_count) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok((block_count * block_size as u64, block_size))
    }

    #[cfg(target_os = "linux")]
    pub fn device_geometry(file: &File) -> io::Result<(u64, u32)> {
        // Valori da <linux/fs.h>: BLKGETSIZE64 = _IOR(0x12, 114, size_t), BLKSSZGET = _IO(0x12, 104)
        const BLKGETSIZE64: libc::c_ulong = 0x8008_1272;
        const BLKSSZGET: libc::c_ulong = 0x1268;
        let fd = file.as_raw_fd();
        let mut size: u64 = 0;
        let mut sector: libc::c_int = 0;
        // SAFETY: ioctl in sola lettura su un fd valido, con buffer della dimensione attesa.
        unsafe {
            if libc::ioctl(fd, BLKGETSIZE64 as _, &mut size) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(fd, BLKSSZGET as _, &mut sector) != 0 {
                sector = 512;
            }
        }
        Ok((size, sector.max(512) as u32))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub fn device_geometry(file: &File) -> io::Result<(u64, u32)> {
        use std::io::{Seek, SeekFrom};
        let mut f = file.try_clone()?;
        let size = f.seek(SeekFrom::End(0))?;
        Ok((size, 512))
    }
}

#[cfg(windows)]
mod platform {
    use std::fs::File;
    use std::io;
    use std::os::windows::fs::FileExt;
    use std::os::windows::io::AsRawHandle;

    pub fn pread(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        file.seek_read(buf, offset)
    }

    pub fn device_geometry(file: &File) -> io::Result<(u64, u32)> {
        use windows_sys::Win32::System::Ioctl::{
            GET_LENGTH_INFORMATION, IOCTL_DISK_GET_LENGTH_INFO,
        };
        use windows_sys::Win32::System::IO::DeviceIoControl;

        let mut info = GET_LENGTH_INFORMATION { Length: 0 };
        let mut returned: u32 = 0;
        // SAFETY: IOCTL di sola lettura su un handle valido, buffer della dimensione corretta.
        let ok = unsafe {
            DeviceIoControl(
                file.as_raw_handle() as _,
                IOCTL_DISK_GET_LENGTH_INFO,
                std::ptr::null(),
                0,
                &mut info as *mut _ as *mut _,
                std::mem::size_of::<GET_LENGTH_INFORMATION>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // TODO: leggere il settore logico con IOCTL_DISK_GET_DRIVE_GEOMETRY_EX.
        Ok((info.Length as u64, 512))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_image_file_read_only() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"0123456789").unwrap();
        let src = ReadOnlySource::open(tmp.path()).unwrap();
        assert_eq!(src.size(), 10);
        assert!(!src.is_device());
        let mut buf = [0u8; 4];
        read_exact_at(&src, 3, &mut buf).unwrap();
        assert_eq!(&buf, b"3456");
        // Oltre la fine: nessun byte.
        assert_eq!(src.read_at(10, &mut buf).unwrap(), 0);
        // A cavallo della fine: lettura parziale.
        assert_eq!(src.read_at(8, &mut buf).unwrap(), 2);
    }
}
