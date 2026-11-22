import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  onEvent,
  type RecoverDone,
  type RecoverProgress,
  type ScanDone,
  type ScanMode,
  type ScanProgress,
  type Selection,
  type SourceInfo,
} from "./api";
import { SourcePicker } from "./SourcePicker";
import { ScanView } from "./ScanView";
import { ResultsView } from "./ResultsView";
import { bytes } from "./format";

type Step = "source" | "scan" | "results";

export default function App() {
  const [step, setStep] = useState<Step>("source");
  const [source, setSource] = useState<SourceInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [scanId, setScanId] = useState(0);
  const [summary, setSummary] = useState<ScanDone | null>(null);
  const [recovering, setRecovering] = useState<RecoverProgress | null>(null);
  const [recovered, setRecovered] = useState<RecoverDone | null>(null);

  useEffect(() => {
    const subs = [
      onEvent<ScanProgress>("scan-progress", setProgress),
      onEvent<ScanDone>("scan-done", (done) => {
        setSummary(done);
        setScanId((n) => n + 1);
        setRunning(false);
        setStep("results");
      }),
      onEvent<RecoverProgress>("recover-progress", setRecovering),
      onEvent<RecoverDone>("recover-done", (d) => {
        setRecovering(null);
        setRecovered(d);
      }),
    ];
    return () => {
      subs.forEach((p) => p.then((un) => un()));
    };
  }, []);

  const openSource = async (path: string) => {
    setError(null);
    try {
      setSource(await api.openSource(path));
      setStep("scan");
    } catch (e) {
      setError(String(e));
    }
  };

  const startScan = async (mode: ScanMode, kinds: string[]) => {
    setProgress(null);
    setRunning(true);
    try {
      await api.startScan(mode, kinds);
    } catch (e) {
      setRunning(false);
      setError(String(e));
    }
  };

  const recover = async (sel: Selection, count: number) => {
    const dest = await open({
      directory: true,
      multiple: false,
      title: "Dove salvare i file recuperati? (scegli un disco diverso da quello analizzato)",
    });
    if (typeof dest !== "string") return;
    try {
      setRecovering({ done: 0, total: count, current: "" });
      await api.recover(sel, dest);
    } catch (e) {
      setRecovering(null);
      setRecovered({ ok: 0, failed: [String(e)], dest });
    }
  };

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="logo" aria-hidden>
            ◐
          </span>
          Sectora
        </div>
        <nav className="steps">
          {(["source", "scan", "results"] as Step[]).map((s, i) => (
            <span key={s} className={`step ${step === s ? "on" : ""}`}>
              {i + 1}. {s === "source" ? "Disco" : s === "scan" ? "Scansione" : "Risultati"}
            </span>
          ))}
        </nav>
        <div className="topbar-right small muted">
          {source && (
            <span className="mono">
              {source.path} · {bytes(source.size)}
            </span>
          )}
        </div>
      </header>

      <div className="content">
        {step === "source" && <SourcePicker onOpen={openSource} error={error} />}
        {step === "scan" && source && (
          <ScanView
            source={source}
            progress={progress}
            running={running}
            onStart={startScan}
            onCancel={() => api.cancel()}
            onBack={() => setStep("source")}
          />
        )}
        {step === "results" && scanId > 0 && (
          <>
            {summary?.cancelled && (
              <div className="alert warn banner">Scansione interrotta: i risultati possono essere incompleti.</div>
            )}
            <ResultsView scanId={scanId} onRecover={recover} onNewScan={() => setStep("scan")} />
          </>
        )}
      </div>

      {(recovering || recovered) && (
        <div className="modal-backdrop">
          <div className="modal">
            {recovering && (
              <>
                <h2>Recupero in corso</h2>
                <div className="bar">
                  <div
                    className="bar-fill"
                    style={{ width: `${(recovering.done / Math.max(1, recovering.total)) * 100}%` }}
                  />
                </div>
                <p className="muted small">
                  {recovering.done} di {recovering.total}
                </p>
                <p className="mono small ellipsis">{recovering.current}</p>
              </>
            )}
            {recovered && (
              <>
                <h2>{recovered.failed.length === 0 ? "Recupero completato" : "Recupero terminato con errori"}</h2>
                <p>
                  {recovered.ok} file salvati in <span className="mono small">{recovered.dest}</span>
                </p>
                {recovered.failed.length > 0 && (
                  <details>
                    <summary>{recovered.failed.length} non recuperati</summary>
                    <ul className="small mono">
                      {recovered.failed.slice(0, 50).map((f) => (
                        <li key={f}>{f}</li>
                      ))}
                    </ul>
                  </details>
                )}
                <div className="actions">
                  <button className="primary" onClick={() => setRecovered(null)}>
                    Chiudi
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
