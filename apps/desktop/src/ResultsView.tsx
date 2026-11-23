import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type Item, type Preview, type Query, type Selection, type TreeNode, type VolumeInfo } from "./api";
import { bytes, categories, conditionInfo, date, integrityInfo } from "./format";

interface Props {
  /** Cambia a ogni nuova scansione: invalida tutte le cache. */
  scanId: number;
  /** Aumenta a ogni aggiornamento dei risultati durante la scansione. */
  liveVersion: number;
  /** Scansione in corso: risultati parziali, recupero disattivato. */
  scanning: boolean;
  onRecover: (sel: Selection, count: number) => void;
  onNewScan: () => void;
}

const ROW = 34;
const PAGE = 300;

function status(item: Item) {
  // L'esito della verifica, quando c'è, conta più dell'ipotesi sui metadati.
  if (item.origin === "fs" && item.deleted && item.integrity !== "unchecked") {
    return integrityInfo[item.integrity];
  }
  return conditionInfo[item.origin === "carved" ? "carved" : (item.condition ?? "intact")];
}

function Badge({ item }: { item: Item }) {
  const info = status(item);
  return (
    <span className={`badge ${info.tone}`} title={info.hint}>
      {info.label}
    </span>
  );
}

/** Ramo dell'albero: i figli vengono chiesti al backend quando si apre. */
function TreeBranch({
  node,
  depth,
  query,
  selected,
  onSelect,
  onRename,
  version,
}: {
  node: TreeNode;
  depth: number;
  query: Query;
  selected: string | null;
  onSelect: (id: string) => void;
  onRename: (id: string, name: string) => Promise<void>;
  version: string;
}) {
  const [open, setOpen] = useState(depth === 0);
  const [children, setChildren] = useState<TreeNode[] | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [renameError, setRenameError] = useState<string | null>(null);
  // Cartelle dei volumi (non la radice del volume né i "Trovati per firma").
  const renamable = /^v\d+\|\/./.test(node.id);

  const startEdit = () => {
    if (!renamable) return;
    setRenameError(null);
    setEditing(node.label);
  };
  const confirm = async () => {
    if (editing === null) return;
    if (editing.trim() === node.label) {
      setEditing(null);
      return;
    }
    try {
      await onRename(node.id, editing);
      setEditing(null);
      setRenameError(null);
    } catch (e) {
      setRenameError(String(e));
    }
  };

  useEffect(() => {
    setChildren(null);
  }, [version]);

  useEffect(() => {
    if (open && children === null && node.has_children) {
      api.tree(query, node.id).then(setChildren).catch(() => setChildren([]));
    }
  }, [open, children, node, query]);

  return (
    <li>
      <div
        className={`tree-row ${selected === node.id ? "active" : ""}`}
        style={{ paddingLeft: 8 + depth * 14 }}
        onClick={() => onSelect(node.id)}
      >
        <span
          className="twisty"
          onClick={(e) => {
            e.stopPropagation();
            setOpen(!open);
          }}
        >
          {node.has_children ? (open ? "▾" : "▸") : ""}
        </span>
        {editing !== null ? (
          <input
            className="tree-input"
            autoFocus
            value={editing}
            onClick={(e) => e.stopPropagation()}
            onChange={(e) => setEditing(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") confirm();
              if (e.key === "Escape") {
                setEditing(null);
                setRenameError(null);
              }
            }}
            onBlur={() => {
              if (!renameError) confirm();
            }}
          />
        ) : (
          <span
            className="tree-label"
            title={renamable ? `${node.label}\nDoppio clic per rinominare` : node.label}
            onDoubleClick={(e) => {
              e.stopPropagation();
              startEdit();
            }}
          >
            {node.label}
          </span>
        )}
        {renamable && editing === null && (
          <button
            className="tree-rename"
            title="Rinomina la cartella: il recupero userà il nuovo nome"
            onClick={(e) => {
              e.stopPropagation();
              startEdit();
            }}
          >
            ✎
          </button>
        )}
        <span className="tree-count">{node.count.toLocaleString("it-IT")}</span>
      </div>
      {renameError && (
        <div className="tree-error small" style={{ paddingLeft: 22 + depth * 14 }}>
          {renameError}
        </div>
      )}
      {open && children && children.length > 0 && (
        <ul className="tree">
          {children.map((c) => (
            <TreeBranch
              key={c.id}
              node={c}
              depth={depth + 1}
              query={query}
              selected={selected}
              onSelect={onSelect}
              onRename={onRename}
              version={version}
            />
          ))}
        </ul>
      )}
    </li>
  );
}

function PreviewPane({ item }: { item: Item | null }) {
  const [preview, setPreview] = useState<Preview | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setPreview(null);
    setError(null);
    if (!item) return;
    let alive = true;
    api
      .preview(item.key)
      .then((p) => alive && setPreview(p))
      .catch((e) => alive && setError(String(e)));
    return () => {
      alive = false;
    };
  }, [item?.key]);

  if (!item) {
    return <aside className="preview empty muted">Seleziona un file per vedere l'anteprima.</aside>;
  }
  return (
    <aside className="preview">
      <div className="preview-box">
        {preview?.kind === "image" && <img src={`data:${preview.mime};base64,${preview.data}`} alt={item.name} />}
        {preview?.kind === "text" && <pre>{preview.data}</pre>}
        {preview?.kind === "none" && <span className="muted small">Anteprima non disponibile per .{item.ext}</span>}
        {!preview && !error && <span className="muted small">Caricamento…</span>}
        {error && <span className="bad-text small">{error}</span>}
      </div>
      <h3 className="preview-name" title={item.name}>
        {item.name}
      </h3>
      <dl className="meta">
        <dt>Stato</dt>
        <dd>
          <Badge item={item} />
        </dd>
        <dt>Dimensione</dt>
        <dd>{bytes(item.size)}</dd>
        <dt>Modificato</dt>
        <dd>{date(item.modified, item.utc_time)}</dd>
        <dt>{item.origin === "carved" ? "Tipo" : "Cartella"}</dt>
        <dd className="mono small">{item.folder}</dd>
      </dl>
      <p className="muted small">{status(item).hint}</p>
      {preview?.truncated && preview.kind === "text" && <p className="muted small">Mostrati i primi 64 KB.</p>}
    </aside>
  );
}

const emptySelection: Selection = { all: null, include: [], exclude: [] };

export function ResultsView({ scanId, liveVersion, scanning, onRecover, onNewScan }: Props) {
  const [query, setQuery] = useState<Query>({
    only_deleted: true,
    category: "all",
    text: "",
    node: null,
    sort: "folder",
    asc: true,
  });
  const [textInput, setTextInput] = useState("");
  const [total, setTotal] = useState(0);
  const [deletedTotal, setDeletedTotal] = useState(0);
  const [counts, setCounts] = useState<Record<string, number>>({});
  const [rows, setRows] = useState<Map<number, Item>>(new Map());
  const [roots, setRoots] = useState<TreeNode[]>([]);
  const [volumes, setVolumes] = useState<VolumeInfo[]>([]);
  const [selection, setSelection] = useState<Selection>(emptySelection);
  const [selInfo, setSelInfo] = useState({ count: 0, size: 0 });
  const [active, setActive] = useState<Item | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(600);
  const tableRef = useRef<HTMLDivElement>(null);
  const loading = useRef<Set<number>>(new Set());
  const queryId = useRef(0);

  // Ricerca: si applica dopo una breve pausa di digitazione.
  useEffect(() => {
    const t = setTimeout(() => setQuery((q) => ({ ...q, text: textInput })), 250);
    return () => clearTimeout(t);
  }, [textInput]);

  useEffect(() => {
    const el = tableRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    api.volumes().then(setVolumes);
    setSelection(emptySelection);
    setActive(null);
  }, [scanId]);

  const loadPage = useCallback(
    async (offset: number, id: number) => {
      const page = Math.floor(offset / PAGE);
      if (loading.current.has(page)) return;
      loading.current.add(page);
      try {
        const res = await api.query(query, page * PAGE, PAGE);
        if (id !== queryId.current) return;
        setTotal(res.total);
        setCounts(res.counts);
        setDeletedTotal(res.deleted_total);
        setRows((prev) => {
          const next = new Map(prev);
          res.rows.forEach((r, i) => next.set(res.offset + i, r));
          return next;
        });
      } finally {
        loading.current.delete(page);
      }
    },
    [query],
  );

  // Nuovi risultati durante la scansione: si ricarica la parte visibile senza
  // tornare in cima e senza perdere la selezione.
  useEffect(() => {
    if (liveVersion === 0) return;
    api.volumes().then(setVolumes);
    queryId.current += 1;
    const id = queryId.current;
    loading.current.clear();
    const offset = Math.floor((tableRef.current?.scrollTop ?? 0) / ROW);
    const page = Math.floor(offset / PAGE);
    api.query(query, page * PAGE, PAGE).then((res) => {
      if (id !== queryId.current) return;
      setTotal(res.total);
      setCounts(res.counts);
      setDeletedTotal(res.deleted_total);
      const next = new Map<number, Item>();
      res.rows.forEach((r, i) => next.set(res.offset + i, r));
      setRows(next);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [liveVersion]);

  // Nuova query: si riparte dalla prima pagina.
  useEffect(() => {
    queryId.current += 1;
    loading.current.clear();
    setRows(new Map());
    tableRef.current?.scrollTo({ top: 0 });
    setScrollTop(0);
    loadPage(0, queryId.current);
  }, [loadPage, scanId]);

  // Albero: le radici dipendono da filtri e categoria, non dalla cartella scelta.
  const treeQuery = useMemo(() => ({ ...query, node: null }), [query]);
  const [renames, setRenames] = useState(0);
  const treeVersion = `${scanId}|${liveVersion}|${renames}|${query.only_deleted}|${query.category}|${query.text}`;
  const renameFolder = useCallback(
    async (id: string, name: string) => {
      const newId = await api.renameFolder(id, name);
      setRenames((n) => n + 1);
      // La cartella aperta (o una sua sottocartella) cambia percorso; le righe si ricaricano.
      setQuery((q) => {
        const node =
          q.node === id ? newId : q.node?.startsWith(id + "/") ? newId + q.node.slice(id.length) : q.node;
        return { ...q, node };
      });
      setActive(null);
    },
    [],
  );
  useEffect(() => {
    api.tree(treeQuery, null).then(setRoots);
  }, [treeVersion]);

  // Righe visibili: si caricano le pagine mancanti mentre si scorre.
  const first = Math.max(0, Math.floor(scrollTop / ROW) - 10);
  const last = Math.min(total, Math.ceil((scrollTop + height) / ROW) + 10);
  useEffect(() => {
    for (let i = first; i < last; i += PAGE) {
      if (!rows.has(i)) loadPage(i, queryId.current);
    }
    if (last > 0 && !rows.has(last - 1)) loadPage(last - 1, queryId.current);
  }, [first, last, rows, loadPage]);

  useEffect(() => {
    const t = setTimeout(() => api.selectionInfo(selection).then(setSelInfo), 120);
    return () => clearTimeout(t);
  }, [selection]);

  const isSelected = (key: string) =>
    selection.all ? !selection.exclude.includes(key) : selection.include.includes(key);
  const toggle = (key: string) => {
    if (selection.all) {
      const ex = selection.exclude.includes(key)
        ? selection.exclude.filter((k) => k !== key)
        : [...selection.exclude, key];
      setSelection({ ...selection, exclude: ex });
    } else {
      const inc = selection.include.includes(key)
        ? selection.include.filter((k) => k !== key)
        : [...selection.include, key];
      setSelection({ ...selection, include: inc });
    }
  };
  const allSelected = selection.all !== null && selection.exclude.length === 0;
  const toggleAll = () => setSelection(allSelected ? emptySelection : { all: query, include: [], exclude: [] });

  const sortBy = (key: Query["sort"]) =>
    setQuery({ ...query, sort: key, asc: query.sort === key ? !query.asc : true });
  const header = (key: Query["sort"], label: string, cls = "") => (
    <div className={`th ${cls} sortable`} onClick={() => sortBy(key)}>
      {label}
      {query.sort === key ? (query.asc ? " ↑" : " ↓") : ""}
    </div>
  );

  const visibleRows: [number, Item | undefined][] = [];
  for (let i = first; i < last; i++) visibleRows.push([i, rows.get(i)]);
  const failedVolumes = volumes.filter((v) => v.error);

  return (
    <div className="results">
      <aside className="sidebar">
        <div className="sidebar-head">
          <strong>Posizioni</strong>
          <button
            className={`link small ${query.node === null ? "active-link" : ""}`}
            onClick={() => setQuery({ ...query, node: null })}
          >
            Tutto
          </button>
        </div>
        <ul className="tree">
          {roots.map((n) => (
            <TreeBranch
              key={n.id}
              node={n}
              depth={0}
              query={treeQuery}
              selected={query.node}
              onSelect={(id) => setQuery({ ...query, node: id })}
              onRename={renameFolder}
              version={treeVersion}
            />
          ))}
        </ul>
        {roots.length > 0 && (
          <div className="sidebar-note small">
            Puoi rinominare le cartelle (✎ o doppio clic), per esempio quelle ritrovate senza nome: il
            recupero ricrea la struttura con i tuoi nomi.
          </div>
        )}
        {failedVolumes.map((v) => (
          <div key={v.index} className="sidebar-note small">
            <strong>{v.name}</strong>: {v.error}
          </div>
        ))}
        <div className="sidebar-foot">
          <button className="link small" onClick={onNewScan}>
            ← Nuova scansione
          </button>
        </div>
      </aside>

      <main className="main">
        <div className="toolbar">
          <div className="tabs">
            {categories.map((c) => (
              <button
                key={c.id}
                className={`tab ${query.category === c.id ? "on" : ""}`}
                onClick={() => setQuery({ ...query, category: c.id })}
                disabled={c.id !== "all" && !counts[c.id]}
              >
                {c.label}
                <span className="tab-count">{(counts[c.id] ?? 0).toLocaleString("it-IT")}</span>
              </button>
            ))}
          </div>
          <div className="toolbar-right">
            <label className="switch small">
              <input
                type="checkbox"
                checked={query.only_deleted}
                onChange={(e) => setQuery({ ...query, only_deleted: e.target.checked })}
              />
              Solo cancellati
            </label>
            <input
              className="search"
              placeholder="Cerca per nome o cartella"
              value={textInput}
              onChange={(e) => setTextInput(e.target.value)}
            />
          </div>
        </div>

        <div className="table">
          <div className="tr thead">
            <div className="th check">
              <input
                type="checkbox"
                checked={allSelected}
                onChange={toggleAll}
                title="Seleziona tutti i file che corrispondono ai filtri"
              />
            </div>
            {header("name", "Nome", "name")}
            {header("folder", "Cartella", "folder")}
            {header("size", "Dimensione", "num")}
            {header("modified", "Modificato", "date")}
            <div className="th state">Stato</div>
          </div>
          <div className="tbody" ref={tableRef} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}>
            {total === 0 ? (
              <div className="empty-state">
                <h2>Nessun file</h2>
                <p className="muted">
                  Nessun file corrisponde ai filtri. Se il disco è un SSD i file cancellati potrebbero essere già
                  stati azzerati; se hai usato la scansione rapida, prova quella completa.
                </p>
              </div>
            ) : (
              <div style={{ height: total * ROW, position: "relative" }}>
                {visibleRows.map(([i, it]) => (
                  <div
                    key={it?.key ?? `p${i}`}
                    className={`tr ${it && active?.key === it.key ? "active" : ""}`}
                    style={{ position: "absolute", top: i * ROW, left: 0, right: 0, height: ROW }}
                    onClick={() => it && setActive(it)}
                  >
                    {it ? (
                      <>
                        <div className="td check" onClick={(e) => e.stopPropagation()}>
                          <input type="checkbox" checked={isSelected(it.key)} onChange={() => toggle(it.key)} />
                        </div>
                        <div className="td name" title={it.name}>
                          <span className={`dot ${it.category}`} />
                          {it.name}
                        </div>
                        <div className="td folder muted" title={it.folder}>
                          {it.folder}
                        </div>
                        <div className="td num">{bytes(it.size)}</div>
                        <div className="td date muted">{date(it.modified, it.utc_time)}</div>
                        <div className="td state">
                          <Badge item={it} />
                        </div>
                      </>
                    ) : (
                      <div className="td muted small placeholder">…</div>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>

        <div className="bottombar">
          <span className="muted small">
            {total.toLocaleString("it-IT")} file mostrati · {deletedTotal.toLocaleString("it-IT")} cancellati
            trovati in totale
          </span>
          <button
            className="primary"
            disabled={selInfo.count === 0 || scanning}
            title={scanning ? "Il recupero si attiva alla fine della scansione" : undefined}
            onClick={() => onRecover(selection, selInfo.count)}
          >
            Recupera {selInfo.count > 0 ? `${selInfo.count.toLocaleString("it-IT")} file (${bytes(selInfo.size)})` : "…"}
          </button>
        </div>
      </main>

      <PreviewPane item={active} />
    </div>
  );
}
