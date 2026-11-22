import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type DeviceInfo } from "./api";
import { bytes } from "./format";

interface Props {
  onOpen: (path: string) => Promise<void>;
  error: string | null;
}

export function SourcePicker({ onOpen, error }: Props) {
  const [devices, setDevices] = useState<DeviceInfo[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const refresh = () => {
    setDevices(null);
    api.listDevices().then(setDevices).catch(() => setDevices([]));
  };
  useEffect(refresh, []);

  const choose = async (path: string) => {
    setBusy(path);
    try {
      await onOpen(path);
    } finally {
      setBusy(null);
    }
  };

  const pickImage = async () => {
    const path = await open({
      multiple: false,
      directory: false,
      title: "Apri un'immagine disco",
      filters: [{ name: "Immagini disco", extensions: ["img", "dd", "raw", "bin", "iso", "dmg"] }],
    });
    if (typeof path === "string") await choose(path);
  };

  const disks = (devices ?? []).filter((d) => !d.is_partition);
  const parts = (devices ?? []).filter((d) => d.is_partition);

  return (
    <div className="page">
      <header className="page-head">
        <h1>Dove hai perso i file?</h1>
        <p className="muted">
          Scegli il disco, la chiavetta o la scheda SD. Il programma la legge in sola lettura: non scrive mai
          niente sul dispositivo da recuperare.
        </p>
      </header>

      {error && <div className="alert bad">{error}</div>}

      <section className="card-grid">
        <button className="source-card image" onClick={pickImage} disabled={busy !== null}>
          <span className="source-icon">⧉</span>
          <span className="source-title">Apri un'immagine disco…</span>
          <span className="muted small">File .img creato con «sectora image» o altri strumenti</span>
        </button>
        {devices === null && <div className="muted">Ricerca dei dischi…</div>}
        {[...disks, ...parts].map((d) => (
          <button
            key={d.path}
            className={`source-card ${d.is_partition ? "partition" : ""}`}
            onClick={() => choose(d.path)}
            disabled={busy !== null}
          >
            <span className="source-icon">{d.removable ? "▭" : "◫"}</span>
            <span className="source-title">
              {d.model || (d.is_partition ? "Partizione" : d.removable ? "Disco rimovibile" : "Disco")}
            </span>
            <span className="muted small mono">{d.path}</span>
            <span className="source-meta">
              <span>{bytes(d.size)}</span>
              {d.is_partition && <span className="chip">partizione</span>}
              {d.removable && <span className="chip">rimovibile</span>}
            </span>
            {d.rotational === false && !d.removable && (
              <span className="warn-text small">
                SSD: i file cancellati potrebbero essere già stati azzerati (TRIM).
              </span>
            )}
            {busy === d.path && <span className="small">Apertura…</span>}
          </button>
        ))}
      </section>

      <footer className="page-foot">
        <button className="link" onClick={refresh}>
          Aggiorna elenco
        </button>
        <span className="muted small">
          Un disco non compare o dà «permesso negato»? Avvia l'app come amministratore, oppure crea prima
          un'immagine con <span className="mono">sudo sectora image</span>.
        </span>
      </footer>
    </div>
  );
}
