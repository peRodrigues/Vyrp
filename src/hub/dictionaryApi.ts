// Espelho TS do dicionário (src-tauri/src/storage/dictionary*.rs) e chamadas aos commands.

import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";

export interface DictionaryEntry {
  id: number;
  phrase: string;
  replacement: string | null;
  replacement_html: string | null;
  is_snippet: boolean;
  manual_entry: boolean;
  origin: string | null;
  observed_asr_error: string | null;
  frequency: number;
  last_used_at: number | null;
  favorite: boolean;
  is_deleted: boolean;
  created_at: number;
}

export interface DictionaryDraft {
  id: number | null;
  phrase: string;
  replacement: string | null;
  is_snippet: boolean;
}

export interface ImportReport {
  added: number;
  updated: number;
  skipped: number;
}

export const emptyDraft = (): DictionaryDraft => ({
  id: null,
  phrase: "",
  replacement: "",
  is_snippet: false,
});

export const draftFromEntry = (entry: DictionaryEntry): DictionaryDraft => ({
  id: entry.id,
  phrase: entry.phrase,
  replacement: entry.replacement ?? "",
  is_snippet: entry.is_snippet,
});

/** Mesmas regras do backend, para avisar antes de enviar. */
export function draftError(draft: DictionaryDraft): string | null {
  if (draft.phrase.trim() === "") return t("dict.err.phrase");
  if (draft.is_snippet && (draft.replacement ?? "").trim() === "") {
    return t("dict.err.snippet");
  }
  return null;
}

export function kindLabel(entry: Pick<DictionaryEntry, "is_snippet" | "replacement">): string {
  if (entry.is_snippet) return t("dict.kind.snippet");
  return entry.replacement ? t("dict.kind.replacement") : t("dict.kind.vocabulary");
}

export function importSummary(report: ImportReport): string {
  const parts = [
    t("dict.import.added", { n: report.added }),
    t("dict.import.updated", { n: report.updated }),
  ];
  if (report.skipped > 0) parts.push(t("dict.import.skipped", { n: report.skipped }));
  return parts.join(", ");
}

export const dictionaryApi = {
  list: (search: string) => invoke<DictionaryEntry[]>("dictionary_list", { search }),
  save: (draft: DictionaryDraft) => invoke<DictionaryEntry>("dictionary_save", { draft }),
  remove: (id: number) => invoke<boolean>("dictionary_delete", { id }),
  setFavorite: (id: number, favorite: boolean) =>
    invoke<boolean>("dictionary_set_favorite", { id, favorite }),
  exportCsv: () => invoke<string>("dictionary_export_csv"),
  importCsv: (csv: string) => invoke<ImportReport>("dictionary_import_csv", { csv }),
};
