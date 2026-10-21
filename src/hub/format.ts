// Funções puras de apresentação do hub (testadas em format.test.ts).

import { decimal, getLanguage, t } from "./i18n";
import type { VoiceAction } from "./settingsApi";

const pad = (n: number) => String(n).padStart(2, "0");

/** "hoje 14:05", "ontem 09:10" ou "20/10 14:05" (com ano se for de outro ano). */
export function formatWhen(timestampMs: number, now: Date = new Date()): string {
  const d = new Date(timestampMs);
  const clock = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diffDays = Math.round((day(now) - day(d)) / 86_400_000);
  if (diffDays === 0) return `${t("time.today")} ${clock}`;
  if (diffDays === 1) return `${t("time.yesterday")} ${clock}`;
  // pt-BR: dia/mês; en: mês/dia.
  const dd = pad(d.getDate());
  const mm = pad(d.getMonth() + 1);
  const date = getLanguage() === "pt-BR" ? `${dd}/${mm}` : `${mm}/${dd}`;
  return d.getFullYear() === now.getFullYear()
    ? `${date} ${clock}`
    : `${date}/${d.getFullYear()} ${clock}`;
}

/** 1800 → "1,8 s"; 65000 → "1 min 05 s". */
export function formatDuration(ms: number): string {
  if (ms < 60_000) return `${decimal((ms / 1000).toFixed(1))} s`;
  const total = Math.round(ms / 1000);
  return `${Math.floor(total / 60)} min ${pad(total % 60)} s`;
}

export function wordsLabel(n: number): string {
  return n === 1 ? t("history.words1") : t("history.words", { n });
}

/** "oi, olá ,, e aí" → ["oi", "olá", "e aí"]. */
export function parsePhrases(text: string): string[] {
  return text
    .split(",")
    .map((p) => p.trim())
    .filter((p) => p !== "");
}

export function describeAction(action: VoiceAction): string {
  if (action.kind === "undo_last") return t("advanced.voice.undo");
  if (action.text === "\n") return t("advanced.voice.insertNewline");
  if (action.text === "\n\n") return t("advanced.voice.insertParagraph");
  return t("advanced.voice.insertText", { text: action.text });
}

/** Modelo "Pequeno" de 466 MB → "466 MB"; acima de 1000, em GB. */
export function formatSize(mb: number): string {
  if (mb < 1000) return t("general.model.size", { mb });
  return `${decimal((mb / 1000).toFixed(1))} GB`;
}

/** Microfones na ordem de preferência: os já ranqueados primeiro (mesmo ausentes), depois os novos. */
export function mergeRanking(ranking: string[], present: string[]): string[] {
  const out = [...new Set(ranking)];
  for (const name of present) if (!out.includes(name)) out.push(name);
  return out;
}

/** Copia a lista movendo o item de `from` para `to` (fora dos limites: sem mudança). */
export function moveItem<T>(list: T[], from: number, to: number): T[] {
  if (from === to || from < 0 || to < 0 || from >= list.length || to >= list.length) return list;
  const next = [...list];
  const [item] = next.splice(from, 1);
  next.splice(to, 0, item);
  return next;
}

/** Tamanho de arquivo em bytes → "466 MB" / "1,5 GB". */
export function formatBytes(bytes: number): string {
  return formatSize(Math.max(0, Math.round(bytes / 1_000_000)));
}
