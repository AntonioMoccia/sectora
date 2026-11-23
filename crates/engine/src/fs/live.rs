//! Anteprima della ricostruzione mentre il disco viene letto.
//!
//! Durante la lettura completa i blocchi di directory del file system
//! precedente passano già in memoria: se ne interpreta subito l'elenco (nessuna
//! lettura in più) e se ne ricava un albero **provvisorio**, mostrato in tempo
//! reale. A fine lettura la ricostruzione vera (con le catene, i blocchi
//! riattaccati e la verifica) lo sostituisce.

use super::{Condition, Extent, FileEntry, Integrity, REBUILT_DIR};
use std::collections::{BTreeMap, HashMap};

/// Voce di un elenco di directory letto durante la scansione.
#[derive(Clone, Debug)]
pub(crate) struct LiveItem {
    pub name: String,
    pub is_dir: bool,
    pub cluster: u32,
    pub size: u64,
    pub modified: Option<i64>,
}

/// Limite delle voci tenute per l'anteprima (memoria su dischi enormi).
const MAX_ITEMS: usize = 2_000_000;

/// Directory ritrovate finora: primo cluster -> (padre se noto, voci).
#[derive(Clone, Debug, Default)]
pub(crate) struct LiveTree {
    dirs: BTreeMap<u32, (Option<u32>, Vec<LiveItem>)>,
    items: usize,
}

impl LiveTree {
    pub(crate) fn add(&mut self, cluster: u32, parent: Option<u32>, items: Vec<LiveItem>) {
        if items.is_empty() || self.items + items.len() > MAX_ITEMS {
            return;
        }
        if let std::collections::btree_map::Entry::Vacant(v) = self.dirs.entry(cluster) {
            self.items += items.len();
            v.insert((parent, items));
        }
    }

    /// Voci provvisorie. `data`: posizione del cluster 2 nel volume.
    pub(crate) fn entries(
        &self,
        data: u64,
        cluster_size: u64,
        cluster_count: u32,
    ) -> Vec<FileEntry> {
        // Nome di una directory: lo dà la voce che la elenca nella directory madre.
        let mut name_of: HashMap<u32, (&str, u32)> = HashMap::new();
        for (&d, (_, items)) in &self.dirs {
            for it in items
                .iter()
                .filter(|it| it.is_dir && it.cluster >= 2 && it.cluster != d)
            {
                name_of.entry(it.cluster).or_insert((it.name.as_str(), d));
            }
        }
        let mut memo: HashMap<u32, String> = HashMap::new();
        let mut out = Vec::new();
        let mut id = 1u64 << 42;
        for (&d, (_, items)) in &self.dirs {
            let dir = self.path(d, &name_of, &mut memo, 0);
            out.push(entry(
                &mut id,
                dir.clone(),
                true,
                0,
                None,
                Vec::new(),
                Condition::Probable,
            ));
            for it in items.iter().filter(|it| !it.is_dir) {
                let valid = it.cluster >= 2
                    && it.cluster < cluster_count + 2
                    && (it.cluster as u64 - 2) * cluster_size + it.size
                        <= cluster_count as u64 * cluster_size;
                let (extents, cond) = if it.size == 0 {
                    (Vec::new(), Condition::Probable)
                } else if valid {
                    let offset = data + (it.cluster as u64 - 2) * cluster_size;
                    (
                        vec![Extent::Disk {
                            offset,
                            length: it.size,
                        }],
                        Condition::Probable,
                    )
                } else {
                    (Vec::new(), Condition::Unsupported)
                };
                let path = format!("{dir}/{}", it.name);
                out.push(entry(
                    &mut id,
                    path,
                    false,
                    it.size,
                    it.modified,
                    extents,
                    cond,
                ));
            }
        }
        out
    }

    /// Percorso provvisorio di una directory: risale i padri finché sono noti.
    fn path(
        &self,
        c: u32,
        name_of: &HashMap<u32, (&str, u32)>,
        memo: &mut HashMap<u32, String>,
        depth: u32,
    ) -> String {
        if let Some(p) = memo.get(&c) {
            return p.clone();
        }
        let top = |c: u32| format!("{REBUILT_DIR}/Cartella ritrovata · blocco {c}");
        let p = if depth > 64 {
            top(c)
        } else if let Some(&(name, parent)) = name_of.get(&c) {
            let base = if self.dirs.contains_key(&parent) {
                self.path(parent, name_of, memo, depth + 1)
            } else {
                top(parent)
            };
            format!("{base}/{name}")
        } else {
            match self.dirs.get(&c).and_then(|d| d.0) {
                Some(parent) if parent >= 2 && self.dirs.contains_key(&parent) => {
                    format!(
                        "{}/Cartella · blocco {c}",
                        self.path(parent, name_of, memo, depth + 1)
                    )
                }
                _ => top(c),
            }
        };
        memo.insert(c, p.clone());
        p
    }
}

fn entry(
    id: &mut u64,
    path: String,
    is_dir: bool,
    size: u64,
    modified: Option<i64>,
    extents: Vec<Extent>,
    condition: Condition,
) -> FileEntry {
    *id += 1;
    FileEntry {
        id: *id,
        name: path.rsplit('/').next().unwrap_or_default().to_string(),
        path,
        size,
        is_dir,
        deleted: true,
        modified,
        condition,
        integrity: Integrity::Unchecked,
        extents,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, is_dir: bool, cluster: u32, size: u64) -> LiveItem {
        LiveItem {
            name: name.into(),
            is_dir,
            cluster,
            size,
            modified: None,
        }
    }

    #[test]
    fn builds_provisional_paths() {
        let mut t = LiveTree::default();
        // Cartella 10 (padre sconosciuto) contiene "Foto" (cluster 20) e un file.
        t.add(
            10,
            Some(0),
            vec![item("Foto", true, 20, 0), item("a.txt", false, 30, 100)],
        );
        t.add(20, Some(10), vec![item("b.jpg", false, 40, 5000)]);
        let e = t.entries(1 << 20, 4096, 1000);
        let paths: Vec<&str> = e.iter().map(|e| e.path.as_str()).collect();
        let top = "/Struttura precedente/Cartella ritrovata · blocco 10";
        assert!(paths.contains(&top));
        assert!(paths.contains(&format!("{top}/a.txt").as_str()));
        assert!(paths.contains(&format!("{top}/Foto/b.jpg").as_str()));
        let b = e.iter().find(|e| e.name == "b.jpg").unwrap();
        assert_eq!(
            b.extents,
            vec![Extent::Disk {
                offset: (1 << 20) + 38 * 4096,
                length: 5000
            }]
        );
    }
}
