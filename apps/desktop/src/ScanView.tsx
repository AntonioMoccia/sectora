import { useState } from "react";
import type { ScanMode, ScanProgress, SourceInfo } from "./api";
import { bytes } from "./format";
import { PHASE_TITLES, determinate as isDeterminate, progressLine, useSpeed } from "./ScanStatus";

const TYPES = [
  { id: "jpg", label: "JPEG" },
  { id: "png", label: "PNG" },
  { id: "gif", label: "GIF" },
  { id: "heic", label: "HEIC" },
  { id: "mp4", label: "MP4" },
  { id: "mov", label: "MOV" },
  { id: "pdf", label: "PDF" },
  { id: "docx", label: "Word" },
  { id: "xlsx", label: "Excel" },
  { id: "pptx", label: "PowerPoint" },
  { id: "zip", label: "ZIP" },
];

const MODES: { id: ScanMode; title: string; text: string }[] = [
  {
    id: "both",
    title: "Completa (consigliata)",
    text: "Legge il file system, ricostruisce le cartelle anche dopo una formattazione, poi cerca i file per firma.",
  },
  {
    id: "quick",
    title: "Rapida",
    text: "Solo file system: pochi secondi, nomi e cartelle originali. Non trova nulla dopo una formattazione.",
  },
  {
    id: "deep",
    title: "Profonda",
    text: "Solo ricerca per firma: funziona anche su dischi formattati o danneggiati, ma senza nomi.",
  },
];

interface Props {
  source: SourceInfo;
  progress: ScanProgress | null;
  running: boolean;
  onStart: (mode: ScanMode, kinds: string[]) => void;
  onCancel: () => void;
  onBack: () => void;
}

export function ScanView({ source, progress, running, onStart, onCancel, onBack }: Props) {
  const [mode, setMode] = useState<ScanMode>("both");
  const [kinds, setKinds] = useState<Set<string>>(new Set());

  const toggle = (id: string) => {
    const next = new Set(kinds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setKinds(next);
  };

  const speed = useSpeed(progress);
  if (running) {
    const phase = progress?.phase ?? "quick";
    const determinate = isDeterminate(progress);
    const pct = progress && progress.total > 0 ? (progress.position / progress.total) * 100 : 0;
    return (
      <div className="page narrow">
        <header className="page-head">
          <h1>{PHASE_TITLES[phase]}</h1>
          <p className="muted mono small">{source.path}</p>
        </header>
        <div className="progress-block">
          <div className="bar">
            <div
              className={`bar-fill ${determinate ? "" : "indeterminate"}`}
              style={{ width: determinate ? `${pct}%` : undefined }}
            />
          </div>
          <div className="progress-stats">
            <span>{progressLine(progress, speed)}</span>
          </div>
          <p className="muted small">I risultati compariranno qui appena disponibili, mentre la scansione continua.</p>
        </div>
        <div className="actions">
          <button className="secondary" onClick={onCancel}>
            Interrompi e mostra i risultati
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="page narrow">
      <header className="page-head">
        <button className="link back" onClick={onBack}>
          ← Cambia disco
        </button>
        <h1>Come vuoi cercare?</h1>
        <p className="muted">
          <span className="mono small">{source.path}</span> · {bytes(source.size)}
          {source.is_device ? " · dispositivo" : " · immagine"}
        </p>
      </header>

      <section className="mode-list">
        {MODES.map((m) => (
          <label key={m.id} className={`mode ${mode === m.id ? "selected" : ""}`}>
            <input type="radio" name="mode" checked={mode === m.id} onChange={() => setMode(m.id)} />
            <span>
              <strong>{m.title}</strong>
              <span className="muted small block">{m.text}</span>
            </span>
          </label>
        ))}
      </section>

      {mode !== "quick" && (
        <section>
          <h3>Tipi da cercare per firma</h3>
          <p className="muted small">Nessuna selezione = tutti. Limitare i tipi riduce i falsi risultati.</p>
          <div className="chips">
            {TYPES.map((t) => (
              <button
                key={t.id}
                className={`chip toggle ${kinds.has(t.id) ? "on" : ""}`}
                onClick={() => toggle(t.id)}
              >
                {t.label}
              </button>
            ))}
          </div>
        </section>
      )}

      <div className="actions">
        <button className="primary" onClick={() => onStart(mode, [...kinds])}>
          Avvia la scansione
        </button>
      </div>
    </div>
  );
}
