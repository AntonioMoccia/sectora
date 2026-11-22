//! Motore di recupero dati.
//!
//! - [`source`]: accesso in sola lettura a dischi e immagini (unico modulo che apre la sorgente)
//! - [`devices`]: elenco di dischi e partizioni
//! - [`carve`]: Deep Scan, ricerca dei file per firma
//! - [`sectora`]: copia dei file trovati e creazione di immagini disco
//!
//! Licenza: GPL-3.0-or-later.

pub mod carve;
pub mod devices;
pub mod fs;
pub mod recover;
pub mod source;

pub use carve::{carve, carve_with, CarveOptions, Event, FileKind, Flow, Found};
pub use fs::{quick_scan, scan_all, Condition, FileEntry, FsType, Integrity, ScanOptions, Volume};
pub use recover::{check_destination, create_image, recover_entry, recover_file, RecoverError};
pub use source::{BlockSource, MemorySource, ReadOnlySource};
