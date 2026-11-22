import type { Condition } from "./api";

export function bytes(n: number | null | undefined): string {
  if (n == null) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let u = 0;
  while (v >= 1000 && u < units.length - 1) {
    v /= 1000;
    u++;
  }
  return u === 0 ? `${n} B` : `${v.toFixed(v < 10 ? 1 : 0)} ${units[u]}`;
}

/**
 * NTFS salva le date in UTC: si convertono nel fuso locale. FAT ed exFAT le salvano già
 * in ora locale del dispositivo: si mostrano così come sono (fuso UTC = nessuna conversione).
 */
export function date(ts: number | null, utc = false): string {
  if (ts == null) return "—";
  return new Date(ts * 1000).toLocaleString("it-IT", {
    timeZone: utc ? undefined : "UTC",
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export const conditionInfo: Record<Condition | "carved", { label: string; tone: string; hint: string }> = {
  intact: { label: "Presente", tone: "neutral", hint: "Il file non è cancellato." },
  recoverable: {
    label: "Recuperabile",
    tone: "good",
    hint: "Cancellato, ma la posizione esatta dei dati è nota.",
  },
  probable: {
    label: "Probabile",
    tone: "warn",
    hint: "Cancellato: si assume che i dati fossero contigui. Di solito è corretto.",
  },
  overwritten: {
    label: "Sovrascritto",
    tone: "bad",
    hint: "Parte dei dati è stata riusata da altri file: il recupero sarà probabilmente danneggiato.",
  },
  unsupported: {
    label: "Non supportato",
    tone: "neutral",
    hint: "File compresso, cifrato o con metadati incompleti.",
  },
  carved: {
    label: "Per firma",
    tone: "info",
    hint: "Trovato dal Deep Scan cercando il formato sul disco: nome e cartella originali non sono noti.",
  },
};

export const integrityInfo = {
  valid: {
    label: "Verificato",
    tone: "good",
    hint: "La struttura del file è completa: il recupero sarà quasi certamente integro.",
  },
  damaged: {
    label: "Danneggiato",
    tone: "bad",
    hint: "Intestazione o struttura non valide: il file era frammentato o è stato sovrascritto. Si recupera la parte leggibile.",
  },
};

export const categories: { id: string; label: string }[] = [
  { id: "all", label: "Tutti" },
  { id: "foto", label: "Foto" },
  { id: "video", label: "Video" },
  { id: "documenti", label: "Documenti" },
  { id: "audio", label: "Audio" },
  { id: "archivi", label: "Archivi" },
  { id: "altro", label: "Altro" },
];
