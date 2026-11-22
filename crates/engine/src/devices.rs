//! Elenco dei dischi e delle partizioni visibili dal sistema.

use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    /// Percorso da passare a `ReadOnlySource::open`.
    pub path: PathBuf,
    /// Dimensione in byte, se leggibile senza privilegi.
    pub size: Option<u64>,
    pub model: Option<String>,
    /// Supporto rimovibile (chiavetta, SD).
    pub removable: Option<bool>,
    /// SSD o flash con TRIM probabile: il recupero dei cancellati è improbabile.
    pub rotational: Option<bool>,
    pub is_partition: bool,
}

/// Elenca i dispositivi. Senza privilegi alcuni campi restano `None`.
pub fn list_devices() -> Vec<DeviceInfo> {
    platform::list()
}

#[cfg(target_os = "linux")]
mod platform {
    use super::DeviceInfo;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn read_trim(p: &Path) -> Option<String> {
        fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn list() -> Vec<DeviceInfo> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir("/sys/block") else {
            return out;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with("ram") && !n.starts_with("zram"))
            .collect();
        names.sort();
        for name in names {
            let base = Path::new("/sys/block").join(&name);
            let sectors: Option<u64> = read_trim(&base.join("size")).and_then(|s| s.parse().ok());
            if sectors == Some(0) {
                continue;
            }
            let removable = read_trim(&base.join("removable")).map(|s| s == "1");
            let rotational = read_trim(&base.join("queue/rotational")).map(|s| s == "1");
            out.push(DeviceInfo {
                path: PathBuf::from("/dev").join(&name),
                size: sectors.map(|s| s * 512),
                model: read_trim(&base.join("device/model")),
                removable,
                rotational,
                is_partition: false,
            });
            if let Ok(parts) = fs::read_dir(&base) {
                let mut parts: Vec<_> = parts
                    .flatten()
                    .filter(|p| p.path().join("partition").exists())
                    .collect();
                parts.sort_by_key(|p| p.file_name());
                for part in parts {
                    let pname = part.file_name().to_string_lossy().into_owned();
                    let psectors: Option<u64> =
                        read_trim(&part.path().join("size")).and_then(|s| s.parse().ok());
                    out.push(DeviceInfo {
                        path: PathBuf::from("/dev").join(&pname),
                        size: psectors.map(|s| s * 512),
                        model: None,
                        removable,
                        rotational,
                        is_partition: true,
                    });
                }
            }
        }
        out
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::DeviceInfo;
    use crate::source::{BlockSource, ReadOnlySource};
    use std::fs;
    use std::path::PathBuf;

    /// `disk2` o `disk2s1`.
    fn parse_name(name: &str) -> Option<bool> {
        let rest = name.strip_prefix("disk")?;
        let (disk, part) = match rest.split_once('s') {
            Some((d, p)) => (d, Some(p)),
            None => (rest, None),
        };
        if disk.is_empty() || !disk.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        match part {
            None => Some(false),
            Some(p) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => Some(true),
            _ => None,
        }
    }

    pub fn list() -> Vec<DeviceInfo> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir("/dev") else {
            return out;
        };
        let mut names: Vec<(String, bool)> = entries
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                parse_name(&n).map(|is_part| (n, is_part))
            })
            .collect();
        names.sort();
        for (name, is_partition) in names {
            // /dev/rdiskN è il dispositivo raw: molto più veloce per letture sequenziali.
            let path = PathBuf::from(format!("/dev/r{name}"));
            let size = ReadOnlySource::open(&path).ok().map(|s| s.size());
            out.push(DeviceInfo {
                path,
                size,
                model: None,
                removable: None,
                rotational: None,
                is_partition,
            });
        }
        out
    }
}

#[cfg(windows)]
mod platform {
    use super::DeviceInfo;
    use crate::source::{BlockSource, ReadOnlySource};
    use std::path::PathBuf;

    pub fn list() -> Vec<DeviceInfo> {
        (0..32)
            .filter_map(|i| {
                let path = PathBuf::from(format!(r"\\.\PhysicalDrive{i}"));
                let src = ReadOnlySource::open(&path).ok()?;
                Some(DeviceInfo {
                    path,
                    size: Some(src.size()),
                    model: None,
                    removable: None,
                    rotational: None,
                    is_partition: false,
                })
            })
            .collect()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::DeviceInfo;
    pub fn list() -> Vec<DeviceInfo> {
        Vec::new()
    }
}
