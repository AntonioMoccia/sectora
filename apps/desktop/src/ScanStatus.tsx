import { useEffect, useRef, useState } from "react";
import type { ScanProgress, ScanUpdate } from "./api";
import { bytes, duration } from "./format";

export const PHASE_TITLES: Record<ScanProgress["phase"], string> = {
  quick: "Lettura del file system",
  deep: "Lettura completa del disco",
  rebuild: "Ricostruzione della struttura delle cartelle",
  finalize: "Unione delle cartelle ritrovate",
  verify: "Verifica dei file trovati",
};

/** Fasi con avanzamento misurabile (le altre mostrano una barra animata). */
export function determinate(p: ScanProgress | null): boolean {
  return !!p && p.total > 0 && p.phase !== "quick";
}

export interface Speed {
  /** Byte (o file, nella verifica) al secondo, sugli ultimi 20 secondi. */
  rate: number;
  /** Secondi rimanenti stimati. */
  eta: number;
}

/** Velocità e tempo rimanente dagli eventi di avanzamento. */
export function useSpeed(progress: ScanProgress | null): Speed | null {
  const samples = useRef<{ phase: string; t: number; pos: number }[]>([]);
  const [speed, setSpeed] = useState<Speed | null>(null);
  useEffect(() => {
    if (!progress || !determinate(progress)) {
      samples.current = [];
      setSpeed(null);
      return;
    }
    const now = performance.now() / 1000;
    const s = samples.current;
    if (s.length && s[0].phase !== progress.phase) s.length = 0;
    s.push({ phase: progress.phase, t: now, pos: progress.position });
    while (s.length > 2 && now - s[0].t > 20) s.shift();
    const first = s[0];
    const dt = now - first.t;
    if (dt < 2) return;
    const rate = (progress.position - first.pos) / dt;
    setSpeed(rate > 0 ? { rate, eta: (progress.total - progress.position) / rate } : null);
  }, [progress]);
  return speed;
}

function rateLabel(p: ScanProgress, speed: Speed | null): string {
  if (!speed) return "calcolo del tempo rimanente…";
  const rate = p.phase === "verify" ? `${Math.round(speed.rate)} file/s` : `${bytes(speed.rate)}/s`;
  return `${rate} · circa ${duration(speed.eta)} alla fine`;
}

/** Descrizione dell'avanzamento: percentuale, quantità, velocità. */
export function progressLine(p: ScanProgress | null, speed: Speed | null): string {
  if (!p) return "Avvio…";
  const pct = p.total > 0 ? ((p.position / p.total) * 100).toFixed(1) : "0";
  switch (p.phase) {
    case "deep":
    case "rebuild":
      return `${pct}% · ${bytes(p.position)} di ${bytes(p.total)} · ${rateLabel(p, speed)}`;
    case "verify":
      return `${p.position.toLocaleString("it-IT")} di ${p.total.toLocaleString("it-IT")} file controllati · ${rateLabel(p, speed)}`;
    case "finalize":
      return "Lettura finita: collego i blocchi delle cartelle sparsi sul disco. Può richiedere qualche minuto su dischi grandi.";
    default:
      return "Analisi delle tabelle del disco…";
  }
}

/** Barra sopra i risultati mentre la scansione continua. */
export function LiveBanner({
  progress,
  update,
  onCancel,
}: {
  progress: ScanProgress | null;
  update: ScanUpdate | null;
  onCancel: () => void;
}) {
  const speed = useSpeed(progress);
  const det = determinate(progress);
  const pct = det && progress ? (progress.position / progress.total) * 100 : 0;
  return (
    <div className="live-banner">
      <div className="live-head">
        <strong>{progress ? PHASE_TITLES[progress.phase] : "Scansione in corso"}</strong>
        <span className="muted small">{progressLine(progress, speed)}</span>
        <button className="secondary small-btn" onClick={onCancel}>
          Interrompi
        </button>
      </div>
      <div className="bar thin">
        <div className={`bar-fill ${det ? "" : "indeterminate"}`} style={{ width: det ? `${pct}%` : undefined }} />
      </div>
      <div className="muted small">
        {update ? `${update.deleted.toLocaleString("it-IT")} file cancellati trovati finora` : ""}
        {update && update.carved > 0 ? ` · ${update.carved.toLocaleString("it-IT")} per firma` : ""}
        {update?.provisional
          ? " · le cartelle ritrovate sono provvisorie: a fine lettura alcune si uniscono o cambiano nome."
          : ""}{" "}
        Il recupero si attiva alla fine della scansione.
      </div>
    </div>
  );
}
