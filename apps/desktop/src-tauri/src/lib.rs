//! Backend dell'app desktop: espone il motore al frontend React tramite
//! comandi Tauri ed eventi di avanzamento.
//!
//! I risultati restano qui: il frontend chiede solo la pagina visibile,
//! i conteggi e i rami dell'albero che apre. Così funziona anche con dischi
//! da milioni di file.

use base64::Engine as _;
use sectora_engine::carve::{carve_with, CarveOptions, Event, FileKind, Flow, Found};
use sectora_engine::devices::{list_devices as engine_devices, DeviceInfo};
use sectora_engine::fs::{
    feed_rebuild, finish_rebuild, read_prefix, scan_volumes, verify_entry, Condition, Extent,
    FileEntry, FsType, Integrity, Volume,
};
use sectora_engine::source::{read_exact_at, BlockSource, ReadOnlySource};
use sectora_engine::{check_destination, recover_entry, recover_file};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

// ---------------------------------------------------------------------------
// Stato
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Inner {
    source: Option<Arc<ReadOnlySource>>,
    volumes: Arc<Vec<Volume>>,
    carved: Arc<Vec<Found>>,
    items: Arc<Vec<Item>>,
    /// Ultima query ordinata: indici in `items`, per scorrere senza riordinare.
    cache: Option<(String, Arc<Vec<u32>>)>,
    busy: bool,
}

#[derive(Default)]
struct AppState {
    inner: Mutex<Inner>,
    cancel: Arc<AtomicBool>,
}

// ---------------------------------------------------------------------------
// Tipi scambiati con il frontend
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct SourceInfo {
    path: String,
    size: u64,
    is_device: bool,
}

#[derive(Serialize, Clone)]
struct ScanProgress {
    /// "quick" | "deep" | "rebuild" | "verify"
    phase: &'static str,
    position: u64,
    total: u64,
    found: usize,
}

#[derive(Serialize, Clone)]
struct ScanDone {
    cancelled: bool,
    volumes: usize,
    files: usize,
    deleted: usize,
    carved: usize,
}

#[derive(Serialize, Clone)]
struct Item {
    key: String,
    origin: &'static str,
    name: String,
    /// Cartella (Quick Scan) o categoria (Deep Scan).
    folder: String,
    volume: Option<usize>,
    size: u64,
    modified: Option<i64>,
    /// NTFS salva le date in UTC; FAT ed exFAT in ora locale del dispositivo.
    utc_time: bool,
    deleted: bool,
    condition: Option<Condition>,
    integrity: Integrity,
    category: &'static str,
    ext: String,
}

#[derive(Serialize)]
struct VolumeInfo {
    index: usize,
    name: String,
    fs: Option<String>,
    size: u64,
    error: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
#[serde(default)]
struct Query {
    only_deleted: bool,
    category: String,
    text: String,
    node: Option<String>,
    sort: String,
    asc: bool,
}

impl Query {
    fn cache_key(&self) -> String {
        format!(
            "{}|{}|{}|{:?}|{}|{}",
            self.only_deleted, self.category, self.text, self.node, self.sort, self.asc
        )
    }
}

#[derive(Serialize)]
struct Page {
    total: usize,
    offset: usize,
    rows: Vec<Item>,
    /// Conteggi per categoria (con gli altri filtri applicati), "all" compreso.
    counts: BTreeMap<String, usize>,
    deleted_total: usize,
}

#[derive(Serialize)]
struct TreeNode {
    id: String,
    label: String,
    count: usize,
    has_children: bool,
}

#[derive(Deserialize, Clone, Default)]
#[serde(default)]
struct Selection {
    /// Tutti i file che corrispondono a questa query...
    all: Option<Query>,
    /// ...più questi...
    include: Vec<String>,
    /// ...meno questi.
    exclude: Vec<String>,
}

#[derive(Serialize)]
struct SelectionInfo {
    count: usize,
    size: u64,
}

#[derive(Serialize)]
struct Preview {
    kind: &'static str, // "image" | "text" | "none"
    mime: String,
    data: String,
    truncated: bool,
}

#[derive(Serialize, Clone)]
struct RecoverProgress {
    done: usize,
    total: usize,
    current: String,
}

#[derive(Serialize, Clone)]
struct RecoverDone {
    ok: usize,
    failed: Vec<String>,
    dest: String,
}

// ---------------------------------------------------------------------------
// Utilità
// ---------------------------------------------------------------------------

fn category(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "heic" | "heif" | "bmp" | "webp" | "tif" | "tiff"
        | "cr2" | "cr3" | "nef" | "arw" | "dng" | "raf" | "orf" | "rw2" => "foto",
        "mp4" | "mov" | "avi" | "mkv" | "m4v" | "3gp" | "mts" | "m2ts" | "wmv" => "video",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "odt" | "ods"
        | "odp" | "rtf" | "csv" | "md" | "pages" | "numbers" | "key" => "documenti",
        "mp3" | "wav" | "m4a" | "flac" | "aac" | "ogg" | "wma" => "audio",
        "zip" | "rar" | "7z" | "gz" | "tar" | "dmg" | "iso" => "archivi",
        _ => "altro",
    }
}

fn ext_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn mime_of(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        // WebKit su macOS mostra le HEIC (foto di iPhone); su Windows e Linux no.
        #[cfg(target_os = "macos")]
        "heic" | "heif" => "image/heic",
        "txt" | "csv" | "md" | "log" | "json" | "xml" | "ini" => "text/plain",
        _ => return None,
    })
}

fn source(state: &State<AppState>) -> Result<Arc<ReadOnlySource>, String> {
    state
        .inner
        .lock()
        .unwrap()
        .source
        .clone()
        .ok_or_else(|| "nessuna sorgente aperta".to_string())
}

fn build_items(volumes: &[Volume], carved: &[Found]) -> Vec<Item> {
    let mut items = Vec::new();
    for (vi, v) in volumes.iter().enumerate() {
        for e in v.entries.iter().filter(|e| !e.is_dir) {
            let ext = ext_of(&e.name);
            let folder = e
                .path
                .rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_default();
            items.push(Item {
                key: format!("v{vi}:{}", e.id),
                origin: "fs",
                name: e.name.clone(),
                folder: if folder.is_empty() {
                    "/".into()
                } else {
                    folder
                },
                volume: Some(vi),
                size: e.size,
                modified: e.modified,
                utc_time: v.fs == Some(FsType::Ntfs),
                deleted: e.deleted,
                condition: Some(e.condition),
                integrity: e.integrity,
                category: category(&ext),
                ext,
            });
        }
    }
    for (ci, f) in carved.iter().enumerate() {
        let ext = f.kind.extension().to_string();
        items.push(Item {
            key: format!("c{ci}"),
            origin: "carved",
            name: format!("f{:014}.{ext}", f.offset),
            folder: f.kind.category().to_string(),
            volume: None,
            size: f.length,
            modified: None,
            utc_time: false,
            deleted: true,
            condition: None,
            integrity: Integrity::Valid, // il Deep Scan accetta solo strutture complete
            category: category(&ext),
            ext,
        });
    }
    items
}

/// Filtro per nodo dell'albero, già interpretato (niente allocazioni per file).
enum NodeFilter {
    All,
    Carved(String),
    Folder {
        volume: usize,
        folder: String,
        prefix: String,
    },
    Nothing,
}

impl NodeFilter {
    fn parse(node: Option<&str>) -> NodeFilter {
        let Some(node) = node else {
            return NodeFilter::All;
        };
        if let Some(cat) = node.strip_prefix("c|") {
            return NodeFilter::Carved(cat.to_string());
        }
        let parsed = node
            .split_once('|')
            .and_then(|(v, f)| Some((v.strip_prefix('v')?.parse::<usize>().ok()?, f)));
        match parsed {
            Some((volume, folder)) => NodeFilter::Folder {
                volume,
                folder: folder.to_string(),
                prefix: format!("{folder}/"),
            },
            None => NodeFilter::Nothing,
        }
    }

    fn matches(&self, item: &Item) -> bool {
        match self {
            NodeFilter::All => true,
            NodeFilter::Nothing => false,
            NodeFilter::Carved(cat) => {
                item.origin == "carved" && (cat.is_empty() || item.category == cat)
            }
            NodeFilter::Folder {
                volume,
                folder,
                prefix,
            } => {
                item.origin == "fs"
                    && item.volume == Some(*volume)
                    && (folder == "/"
                        || item.folder == *folder
                        || item.folder.starts_with(prefix.as_str()))
            }
        }
    }
}

/// Filtri comuni (tutto tranne categoria e cartella).
fn base_match(item: &Item, q: &Query, text: &str) -> bool {
    (!q.only_deleted || item.deleted)
        && (text.is_empty()
            || item.name.to_lowercase().contains(text)
            || item.folder.to_lowercase().contains(text))
}

fn matches(item: &Item, q: &Query, text: &str, node: &NodeFilter) -> bool {
    base_match(item, q, text)
        && (q.category.is_empty() || q.category == "all" || item.category == q.category)
        && node.matches(item)
}

fn sorted_indices(items: &[Item], q: &Query) -> Vec<u32> {
    let text = q.text.trim().to_lowercase();
    let node = NodeFilter::parse(q.node.as_deref());
    let mut idx: Vec<u32> = (0..items.len() as u32)
        .filter(|i| matches(&items[*i as usize], q, &text, &node))
        .collect();
    let by = |a: &Item, b: &Item| match q.sort.as_str() {
        "size" => a.size.cmp(&b.size),
        "modified" => a.modified.cmp(&b.modified),
        "name" => a.name.cmp(&b.name),
        _ => a.folder.cmp(&b.folder).then_with(|| a.name.cmp(&b.name)),
    };
    idx.sort_by(|a, b| {
        let o = by(&items[*a as usize], &items[*b as usize]);
        if q.asc {
            o
        } else {
            o.reverse()
        }
    });
    idx
}

fn selected<'a>(items: &'a [Item], sel: &Selection) -> Vec<&'a Item> {
    let include: HashSet<&str> = sel.include.iter().map(String::as_str).collect();
    let exclude: HashSet<&str> = sel.exclude.iter().map(String::as_str).collect();
    let all_text = sel
        .all
        .as_ref()
        .map(|q| q.text.trim().to_lowercase())
        .unwrap_or_default();
    let all_node = NodeFilter::parse(sel.all.as_ref().and_then(|q| q.node.as_deref()));
    items
        .iter()
        .filter(|i| {
            if exclude.contains(i.key.as_str()) {
                return false;
            }
            include.contains(i.key.as_str())
                || sel
                    .all
                    .as_ref()
                    .is_some_and(|q| matches(i, q, &all_text, &all_node))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Comandi
// ---------------------------------------------------------------------------

#[tauri::command]
fn list_devices() -> Vec<DeviceInfo> {
    engine_devices()
}

#[tauri::command]
fn open_source(path: String, state: State<AppState>) -> Result<SourceInfo, String> {
    let src = ReadOnlySource::open(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!(
                "Permesso negato su {path}. Per leggere un disco serve avviare l'app come amministratore \
                 (macOS/Linux: sudo, Windows: \"Esegui come amministratore\"). In alternativa apri un'immagine disco."
            )
        } else {
            format!("Impossibile aprire {path}: {e}")
        }
    })?;
    let info = SourceInfo {
        path: path.clone(),
        size: src.size(),
        is_device: src.is_device(),
    };
    let mut inner = state.inner.lock().unwrap();
    if inner.busy {
        return Err("una scansione è in corso".into());
    }
    *inner = Inner {
        source: Some(Arc::new(src)),
        ..Default::default()
    };
    Ok(info)
}

/// Avvia la scansione in un thread. `mode`: "quick", "deep" o "both".
///
/// "both" legge il disco una sola volta: durante il Deep Scan gli stessi blocchi
/// alimentano la ricostruzione delle strutture precedenti a una formattazione.
#[tauri::command]
fn start_scan(
    mode: String,
    kinds: Vec<String>,
    app: AppHandle,
    state: State<AppState>,
) -> Result<(), String> {
    let src = source(&state)?;
    {
        let mut inner = state.inner.lock().unwrap();
        if inner.busy {
            return Err("una scansione è già in corso".into());
        }
        inner.busy = true;
        inner.volumes = Arc::new(Vec::new());
        inner.carved = Arc::new(Vec::new());
        inner.items = Arc::new(Vec::new());
        inner.cache = None;
    }
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::SeqCst);
    let kinds: BTreeSet<FileKind> = kinds
        .iter()
        .filter_map(|k| FileKind::from_extension(k))
        .collect();

    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let src: &dyn BlockSource = src.as_ref();
        let total = src.size();
        let emit = |phase: &'static str, position: u64, total: u64, found: usize| {
            let _ = app.emit(
                "scan-progress",
                ScanProgress {
                    phase,
                    position,
                    total,
                    found,
                },
            );
        };
        let stopped = || cancel.load(Ordering::SeqCst);

        // 1. File system (veloce: tabelle e metadati).
        let mut volumes = Vec::new();
        if mode == "quick" || mode == "both" {
            emit("quick", 0, total, 0);
            volumes = scan_volumes(src);
        }

        // 2. Lettura completa: Deep Scan e/o ricostruzione nella stessa passata.
        let mut carved = Vec::new();
        let deep = mode == "deep" || mode == "both";
        let rebuild = mode == "both";
        if (deep || rebuild) && !stopped() {
            let opts = CarveOptions {
                kinds,
                detect: deep,
                ..Default::default()
            };
            let phase = if deep { "deep" } else { "rebuild" };
            let mut last = Instant::now() - Duration::from_secs(1);
            let mut found = 0usize;
            carved = carve_with(
                src,
                &opts,
                |ev| {
                    match ev {
                        Event::Progress { position, total } => {
                            if last.elapsed() > Duration::from_millis(150) || position >= total {
                                last = Instant::now();
                                emit(phase, *position, *total, found);
                            }
                        }
                        Event::Found(_) => found += 1,
                        Event::ReadError { .. } => {}
                    }
                    if stopped() {
                        Flow::Stop
                    } else {
                        Flow::Continue
                    }
                },
                &mut |abs, chunk| {
                    if rebuild {
                        feed_rebuild(src, &mut volumes, abs, chunk)
                    }
                },
            );
            if rebuild {
                finish_rebuild(src, &mut volumes);
            }
        }

        // I file già trovati dal file system non si ripetono tra quelli trovati per firma.
        let known: HashSet<u64> = volumes
            .iter()
            .flat_map(|v| {
                v.entries
                    .iter()
                    .filter_map(move |e| match e.extents.first() {
                        Some(Extent::Disk { offset, .. }) if !e.is_dir => Some(v.offset + offset),
                        _ => None,
                    })
            })
            .collect();
        carved.retain(|f| !known.contains(&f.offset));

        // 3. Verifica del contenuto dei file cancellati.
        let to_check: usize = volumes
            .iter()
            .flat_map(|v| &v.entries)
            .filter(|e| e.deleted && !e.is_dir && e.condition != Condition::Unsupported)
            .count();
        let mut checked = 0usize;
        let mut last = Instant::now() - Duration::from_secs(1);
        'verify: for vol in volumes.iter_mut() {
            let meta = vol.clone_meta();
            for e in vol.entries.iter_mut() {
                if !e.deleted || e.is_dir || e.condition == Condition::Unsupported {
                    continue;
                }
                if stopped() {
                    break 'verify;
                }
                e.integrity = verify_entry(src, &meta, e);
                checked += 1;
                if last.elapsed() > Duration::from_millis(150) {
                    last = Instant::now();
                    emit("verify", checked as u64, to_check as u64, 0);
                }
            }
        }

        let items = build_items(&volumes, &carved);
        let files = volumes
            .iter()
            .flat_map(|v| &v.entries)
            .filter(|e| !e.is_dir)
            .count();
        let deleted = volumes
            .iter()
            .flat_map(|v| &v.entries)
            .filter(|e| !e.is_dir && e.deleted)
            .count();
        let done = ScanDone {
            cancelled: stopped(),
            volumes: volumes.len(),
            files,
            deleted,
            carved: carved.len(),
        };
        {
            let mut inner = state.inner.lock().unwrap();
            inner.volumes = Arc::new(volumes);
            inner.carved = Arc::new(carved);
            inner.items = Arc::new(items);
            inner.cache = None;
            inner.busy = false;
        }
        let _ = app.emit("scan-done", done);
    });
    Ok(())
}

#[tauri::command]
fn cancel(state: State<AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn volumes(state: State<AppState>) -> Vec<VolumeInfo> {
    let inner = state.inner.lock().unwrap();
    inner
        .volumes
        .iter()
        .enumerate()
        .map(|(i, v)| VolumeInfo {
            index: i,
            name: v.display_name(),
            fs: v.fs.map(|f| f.name().to_string()),
            size: v.size,
            error: v.error.clone(),
        })
        .collect()
}

/// Una pagina di risultati con i conteggi per categoria.
#[tauri::command]
fn query(q: Query, offset: usize, limit: usize, state: State<AppState>) -> Page {
    let (items, cached) = {
        let inner = state.inner.lock().unwrap();
        let key = q.cache_key();
        let cached = inner
            .cache
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.clone());
        (inner.items.clone(), cached)
    };
    let order = match cached {
        Some(o) => o,
        None => {
            let o = Arc::new(sorted_indices(&items, &q));
            state.inner.lock().unwrap().cache = Some((q.cache_key(), o.clone()));
            o
        }
    };
    let text = q.text.trim().to_lowercase();
    let node = NodeFilter::parse(q.node.as_deref());
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for it in items.iter() {
        if base_match(it, &q, &text) && node.matches(it) {
            *counts.entry("all".into()).or_default() += 1;
            *counts.entry(it.category.to_string()).or_default() += 1;
        }
    }
    let rows = order
        .iter()
        .skip(offset)
        .take(limit.min(2000))
        .map(|i| items[*i as usize].clone())
        .collect();
    Page {
        total: order.len(),
        offset,
        rows,
        counts,
        deleted_total: items.iter().filter(|i| i.deleted).count(),
    }
}

/// Figli di un nodo dell'albero (None = radici), con i conteggi filtrati.
#[tauri::command]
fn tree(q: Query, parent: Option<String>, state: State<AppState>) -> Vec<TreeNode> {
    let (items, volumes) = {
        let inner = state.inner.lock().unwrap();
        (inner.items.clone(), inner.volumes.clone())
    };
    let text = q.text.trim().to_lowercase();
    let visible = |it: &Item| {
        base_match(it, &q, &text)
            && (q.category.is_empty() || q.category == "all" || it.category == q.category)
    };

    match parent.as_deref() {
        None => {
            let mut out = Vec::new();
            for (vi, v) in volumes.iter().enumerate() {
                let count = items
                    .iter()
                    .filter(|i| i.volume == Some(vi) && i.origin == "fs" && visible(i))
                    .count();
                out.push(TreeNode {
                    id: format!("v{vi}|/"),
                    label: v.display_name(),
                    count,
                    has_children: count > 0,
                });
            }
            let carved = items
                .iter()
                .filter(|i| i.origin == "carved" && visible(i))
                .count();
            if carved > 0 {
                out.push(TreeNode {
                    id: "c|".into(),
                    label: "Trovati per firma".into(),
                    count: carved,
                    has_children: true,
                });
            }
            out
        }
        Some("c|") => {
            let mut by: BTreeMap<&str, usize> = BTreeMap::new();
            for it in items.iter().filter(|i| i.origin == "carved" && visible(i)) {
                *by.entry(it.category).or_default() += 1;
            }
            by.into_iter()
                .map(|(cat, count)| TreeNode {
                    id: format!("c|{cat}"),
                    label: cat[..1].to_uppercase() + &cat[1..],
                    count,
                    has_children: false,
                })
                .collect()
        }
        Some(node) => {
            let Some((vol, folder)) = node.split_once('|') else {
                return Vec::new();
            };
            let prefix = if folder == "/" {
                "/".to_string()
            } else {
                format!("{folder}/")
            };
            // nome figlio -> (conteggio, ha sottocartelle)
            let mut children: BTreeMap<String, (usize, bool)> = BTreeMap::new();
            let vol_index: Option<usize> = vol.strip_prefix('v').and_then(|v| v.parse().ok());
            for it in items.iter() {
                if it.origin != "fs" || it.volume != vol_index || !visible(it) {
                    continue;
                }
                let Some(rest) = it.folder.strip_prefix(&prefix) else {
                    continue;
                };
                if rest.is_empty() {
                    continue;
                }
                let (child, deeper) = match rest.split_once('/') {
                    Some((c, _)) => (c, true),
                    None => (rest, false),
                };
                let e = children.entry(child.to_string()).or_default();
                e.0 += 1;
                e.1 |= deeper;
            }
            children
                .into_iter()
                .map(|(name, (count, deeper))| TreeNode {
                    id: format!("{vol}|{prefix}{name}"),
                    label: name,
                    count,
                    has_children: deeper,
                })
                .collect()
        }
    }
}

#[tauri::command]
fn selection_info(sel: Selection, state: State<AppState>) -> SelectionInfo {
    let items = state.inner.lock().unwrap().items.clone();
    let chosen = selected(&items, &sel);
    SelectionInfo {
        count: chosen.len(),
        size: chosen.iter().map(|i| i.size).sum(),
    }
}

enum Target<'a> {
    Fs(&'a Volume, &'a FileEntry),
    Carved(&'a Found),
}

fn resolve<'a>(key: &str, volumes: &'a [Volume], carved: &'a [Found]) -> Option<Target<'a>> {
    if let Some(rest) = key.strip_prefix('v') {
        let (vi, id) = rest.split_once(':')?;
        let vol = volumes.get(vi.parse::<usize>().ok()?)?;
        let id: u64 = id.parse().ok()?;
        let entry = vol.entries.iter().find(|e| e.id == id)?;
        return Some(Target::Fs(vol, entry));
    }
    let ci: usize = key.strip_prefix('c')?.parse().ok()?;
    carved.get(ci).map(Target::Carved)
}

#[tauri::command]
fn preview(key: String, state: State<AppState>) -> Result<Preview, String> {
    const MAX_IMAGE: usize = 30 * 1024 * 1024;
    const MAX_TEXT: usize = 64 * 1024;
    let src = source(&state)?;
    let (volumes, carved) = {
        let inner = state.inner.lock().unwrap();
        (inner.volumes.clone(), inner.carved.clone())
    };
    let none = || Preview {
        kind: "none",
        mime: String::new(),
        data: String::new(),
        truncated: false,
    };
    let target = resolve(&key, &volumes, &carved).ok_or("elemento non trovato")?;
    let (name, size) = match &target {
        Target::Fs(_, e) => (e.name.clone(), e.size),
        Target::Carved(f) => (format!("x.{}", f.kind.extension()), f.length),
    };
    let ext = ext_of(&name);
    let Some(mime) = mime_of(&ext) else {
        return Ok(none());
    };
    let is_text = mime.starts_with("text/");
    let max = if is_text { MAX_TEXT } else { MAX_IMAGE };
    if !is_text && size as usize > max {
        return Ok(none());
    }
    let bytes = match target {
        Target::Fs(vol, e) => read_prefix(src.as_ref(), vol, e, max).map_err(|e| e.to_string())?,
        Target::Carved(f) => {
            let mut buf = vec![0u8; (f.length as usize).min(max)];
            read_exact_at(src.as_ref(), f.offset, &mut buf).map_err(|e| e.to_string())?;
            buf
        }
    };
    let truncated = (bytes.len() as u64) < size;
    Ok(if is_text {
        Preview {
            kind: "text",
            mime: mime.into(),
            data: String::from_utf8_lossy(&bytes).into_owned(),
            truncated,
        }
    } else {
        Preview {
            kind: "image",
            mime: mime.into(),
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
            truncated,
        }
    })
}

#[tauri::command]
fn recover(
    sel: Selection,
    dest: String,
    app: AppHandle,
    state: State<AppState>,
) -> Result<(), String> {
    let src = source(&state)?;
    let dest = PathBuf::from(dest);
    check_destination(&src, &dest).map_err(|e| e.to_string())?;
    let (volumes, carved, items) = {
        let inner = state.inner.lock().unwrap();
        (
            inner.volumes.clone(),
            inner.carved.clone(),
            inner.items.clone(),
        )
    };
    let keys: Vec<String> = selected(&items, &sel)
        .into_iter()
        .map(|i| i.key.clone())
        .collect();
    std::thread::spawn(move || {
        let total = keys.len();
        let mut ok = 0;
        let mut failed = Vec::new();
        let mut last = Instant::now() - Duration::from_secs(1);
        for (i, key) in keys.iter().enumerate() {
            let result = match resolve(key, &volumes, &carved) {
                Some(Target::Fs(vol, e)) => {
                    recover_entry(src.as_ref(), vol, e, &dest).map(|_| e.path.clone())
                }
                Some(Target::Carved(f)) => {
                    recover_file(src.as_ref(), f, &dest.join("Trovati per firma"))
                        .map(|p| p.display().to_string())
                }
                None => Err(std::io::Error::other("elemento non trovato")),
            };
            match result {
                Ok(name) => {
                    ok += 1;
                    if last.elapsed() > Duration::from_millis(100) || i + 1 == total {
                        last = Instant::now();
                        let _ = app.emit(
                            "recover-progress",
                            RecoverProgress {
                                done: i + 1,
                                total,
                                current: name,
                            },
                        );
                    }
                }
                Err(e) => failed.push(format!("{key}: {e}")),
            }
        }
        let _ = app.emit(
            "recover-done",
            RecoverDone {
                ok,
                failed,
                dest: dest.display().to_string(),
            },
        );
    });
    Ok(())
}

/// Controlla un nome di cartella: deve essere valido su Windows, macOS e Linux,
/// perché il recupero la ricrea con questo nome.
fn valid_folder_name(name: &str) -> Result<String, String> {
    let name = name.trim().trim_end_matches('.').trim_end().to_string();
    if name.is_empty() || name == "." || name == ".." {
        return Err("Scrivi un nome per la cartella.".into());
    }
    if name.chars().count() > 200 {
        return Err("Il nome è troppo lungo (massimo 200 caratteri).".into());
    }
    if let Some(c) = name
        .chars()
        .find(|c| c.is_control() || "/\\:*?\"<>|".contains(*c))
    {
        return Err(format!("Il nome non può contenere il carattere {c:?}."));
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Err(format!("\"{name}\" è un nome riservato da Windows."));
    }
    Ok(name)
}

/// Rinomina la cartella `old` (percorso) del volume `vi` in `name`: cambia il
/// percorso di tutto il suo contenuto. Ritorna il nuovo percorso.
fn rename_paths(
    entries: &mut [FileEntry],
    items: &mut [Item],
    vi: usize,
    old: &str,
    name: &str,
) -> Result<String, String> {
    let parent = old.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let new = format!("{parent}/{name}");
    if new == old {
        return Ok(new);
    }
    let old_prefix = format!("{old}/");
    let new_prefix = format!("{new}/");
    let under = |p: &str| p == old || p.starts_with(&old_prefix);
    let mine = |i: &Item| i.volume == Some(vi) && i.origin == "fs";
    if !entries.iter().any(|e| under(&e.path)) && !items.iter().any(|i| mine(i) && under(&i.folder))
    {
        return Err("Cartella non trovata.".into());
    }
    // Windows e macOS non distinguono maiuscole e minuscole: "foto" e "Foto" coincidono,
    // salvo che si stia solo cambiando maiuscole/minuscole della stessa cartella.
    let (new_l, new_prefix_l) = (new.to_lowercase(), new_prefix.to_lowercase());
    let taken = |p: &str| {
        let l = p.to_lowercase();
        (l == new_l || l.starts_with(&new_prefix_l)) && !under(p)
    };
    if entries.iter().any(|e| taken(&e.path)) || items.iter().any(|i| mine(i) && taken(&i.folder)) {
        return Err(format!(
            "Esiste già una cartella \"{name}\" in questa posizione."
        ));
    }
    let rename = |p: &mut String| {
        if p == old {
            *p = new.clone();
        } else if let Some(rest) = p.strip_prefix(&old_prefix) {
            *p = format!("{new_prefix}{rest}");
        }
    };
    for e in entries.iter_mut() {
        if e.path == old {
            e.name = name.to_string();
        }
        rename(&mut e.path);
    }
    for it in items.iter_mut().filter(|i| mine(i)) {
        rename(&mut it.folder);
    }
    Ok(new)
}

/// Rinomina una cartella dei risultati (es. una "Cartella ritrovata") prima del
/// recupero: il recupero ricrea la struttura con il nuovo nome. Ritorna l'id del
/// nodo rinominato.
#[tauri::command]
fn rename_folder(node: String, name: String, state: State<AppState>) -> Result<String, String> {
    let name = valid_folder_name(&name)?;
    let (vol, old) = node
        .split_once('|')
        .filter(|(v, f)| v.starts_with('v') && f.starts_with('/') && *f != "/")
        .ok_or("Si possono rinominare solo le cartelle.")?;
    let vi: usize = vol[1..].parse().map_err(|_| "volume non valido")?;
    let mut guard = state.inner.lock().unwrap();
    let inner = &mut *guard;
    if inner.busy {
        return Err("Attendi la fine della scansione.".into());
    }
    if vi >= inner.volumes.len() {
        return Err("volume non valido".into());
    }
    let volumes = Arc::make_mut(&mut inner.volumes);
    let items = Arc::make_mut(&mut inner.items);
    let new = rename_paths(&mut volumes[vi].entries, items, vi, old, &name)?;
    inner.cache = None;
    Ok(format!("{vol}|{new}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_devices,
            open_source,
            start_scan,
            cancel,
            volumes,
            query,
            tree,
            selection_info,
            preview,
            recover,
            rename_folder
        ])
        .run(tauri::generate_context!())
        .expect("errore all'avvio dell'app");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str, folder: &str, cat: &'static str, deleted: bool, size: u64) -> Item {
        Item {
            key: key.into(),
            origin: "fs",
            name: format!("{key}.jpg"),
            folder: folder.into(),
            volume: Some(0),
            size,
            modified: None,
            utc_time: false,
            deleted,
            condition: Some(Condition::Recoverable),
            integrity: Integrity::Unchecked,
            category: cat,
            ext: "jpg".into(),
        }
    }

    #[test]
    fn rename_moves_the_whole_folder() {
        let entry = |id: u64, path: &str, is_dir: bool| FileEntry {
            id,
            path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
            size: 1,
            is_dir,
            deleted: true,
            modified: None,
            condition: Condition::Probable,
            integrity: Integrity::Unchecked,
            extents: Vec::new(),
        };
        let base = "/Struttura precedente/Cartella ritrovata 1 (3 foto)";
        let mut entries = vec![
            entry(1, base, true),
            entry(2, &format!("{base}/Foto"), true),
            entry(3, &format!("{base}/Foto/a.jpg"), false),
            entry(4, &format!("{base}/b.jpg"), false),
            entry(
                5,
                "/Struttura precedente/Cartella ritrovata 10 (1 foto)/c.jpg",
                false,
            ),
            entry(6, "/Struttura precedente/Lavoro/d.pdf", false),
        ];
        let mut items = vec![
            item("v0:3", &format!("{base}/Foto"), "foto", true, 1),
            item("v0:4", base, "foto", true, 1),
            item(
                "v0:5",
                "/Struttura precedente/Cartella ritrovata 10 (1 foto)",
                "foto",
                true,
                1,
            ),
        ];
        let new = rename_paths(&mut entries, &mut items, 0, base, "Vacanze 2024").unwrap();
        assert_eq!(new, "/Struttura precedente/Vacanze 2024");
        assert_eq!(entries[0].name, "Vacanze 2024");
        assert_eq!(
            entries[2].path,
            "/Struttura precedente/Vacanze 2024/Foto/a.jpg"
        );
        assert_eq!(items[0].folder, "/Struttura precedente/Vacanze 2024/Foto");
        assert_eq!(items[1].folder, "/Struttura precedente/Vacanze 2024");
        // Le altre cartelle, anche con un nome che inizia allo stesso modo, non cambiano.
        assert!(entries[4].path.contains("Cartella ritrovata 10"));
        assert_eq!(
            items[2].folder,
            "/Struttura precedente/Cartella ritrovata 10 (1 foto)"
        );
        // Nome già usato (anche con maiuscole diverse) e cartella inesistente.
        assert!(rename_paths(&mut entries, &mut items, 0, &new, "lavoro").is_err());
        assert!(rename_paths(&mut entries, &mut items, 0, "/Nessuna", "X").is_err());
        // Solo maiuscole/minuscole della stessa cartella: consentito.
        assert!(rename_paths(&mut entries, &mut items, 0, &new, "VACANZE 2024").is_ok());
    }

    #[test]
    fn folder_names_are_checked() {
        assert_eq!(
            valid_folder_name("  Vacanze 2024  ").unwrap(),
            "Vacanze 2024"
        );
        assert_eq!(valid_folder_name("Foto.").unwrap(), "Foto");
        for bad in [
            "", "  ", "..", "a/b", "a\\b", "che?", "CON", "com1", "lpt9.txt", "x\u{7}",
        ] {
            assert!(
                valid_folder_name(bad).is_err(),
                "{bad:?} dovrebbe essere rifiutato"
            );
        }
        assert!(valid_folder_name("Console").is_ok());
    }

    #[test]
    fn query_filters_node_and_selection() {
        let items = vec![
            item("a", "/DCIM", "foto", true, 10),
            item("b", "/DCIM/2025", "foto", true, 20),
            item("c", "/Documenti", "documenti", true, 30),
            item("d", "/DCIM", "foto", false, 40),
        ];
        let q = Query {
            only_deleted: true,
            category: "all".into(),
            node: Some("v0|/DCIM".into()),
            sort: "size".into(),
            asc: false,
            ..Default::default()
        };
        let order = sorted_indices(&items, &q);
        assert_eq!(
            order,
            vec![1, 0],
            "solo cancellati sotto /DCIM, per dimensione decrescente"
        );

        let sel = Selection {
            all: Some(q),
            include: vec!["c".into()],
            exclude: vec!["b".into()],
        };
        let keys: Vec<&str> = selected(&items, &sel)
            .iter()
            .map(|i| i.key.as_str())
            .collect();
        assert_eq!(keys, vec!["a", "c"]);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    /// `cargo test --release -p sectora-desktop -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn one_million_files() {
        let cats = ["foto", "video", "documenti", "altro"];
        let items: Vec<Item> = (0..1_000_000u32)
            .map(|i| Item {
                key: format!("v0:{i}"),
                origin: "fs",
                name: format!("IMG_{i:07}.jpg"),
                folder: format!("/DCIM/cartella {:04}/sotto {}", i % 5000, i % 7),
                volume: Some(0),
                size: (i as u64 * 7919) % 5_000_000,
                modified: Some(i as i64),
                utc_time: true,
                deleted: i % 3 == 0,
                condition: Some(Condition::Recoverable),
                integrity: Integrity::Unchecked,
                category: cats[(i % 4) as usize],
                ext: "jpg".into(),
            })
            .collect();
        let q = Query {
            only_deleted: true,
            category: "all".into(),
            sort: "size".into(),
            asc: false,
            ..Default::default()
        };
        let t = Instant::now();
        let order = sorted_indices(&items, &q);
        println!(
            "filtro + ordinamento: {} file in {:?}",
            order.len(),
            t.elapsed()
        );
        let t = Instant::now();
        let sel = Selection {
            all: Some(q),
            include: vec![],
            exclude: vec!["v0:3".into()],
        };
        let n = selected(&items, &sel).len();
        println!("selezione per filtro: {n} file in {:?}", t.elapsed());
        let t = Instant::now();
        let text = String::new();
        let node = NodeFilter::parse(Some("v0|/DCIM/cartella 0042"));
        let mut c = 0;
        for it in &items {
            if base_match(it, &Query::default(), &text) && node.matches(it) {
                c += 1;
            }
        }
        println!("conteggio cartella: {c} file in {:?}", t.elapsed());
    }
}
