import { useCallback, useEffect, useRef, useState } from "react";
import {
  dictionaryApi,
  draftError,
  draftFromEntry,
  emptyDraft,
  importSummary,
  kindLabel,
} from "./dictionaryApi";
import { t } from "./i18n";
import { errorText } from "./useSettings";
import type { DictionaryDraft, DictionaryEntry } from "./dictionaryApi";


export default function Dictionary() {
  const [entries, setEntries] = useState<DictionaryEntry[]>([]);
  const [search, setSearch] = useState("");
  const [draft, setDraft] = useState<DictionaryDraft | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmingId, setConfirmingId] = useState<number | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);

  const refresh = useCallback(() => {
    dictionaryApi
      .list(search)
      .then(setEntries)
      .catch((err: unknown) => setNotice(errorText(err)));
  }, [search]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const save = () => {
    if (!draft) return;
    const problem = draftError(draft);
    if (problem) {
      setFormError(problem);
      return;
    }
    dictionaryApi
      .save(draft)
      .then(() => {
        setDraft(null);
        setFormError(null);
        refresh();
      })
      .catch((err: unknown) => setFormError(errorText(err)));
  };

  const remove = (id: number) => {
    dictionaryApi
      .remove(id)
      .then(() => {
        setConfirmingId(null);
        refresh();
      })
      .catch((err: unknown) => setNotice(errorText(err)));
  };

  const toggleFavorite = (entry: DictionaryEntry) => {
    dictionaryApi
      .setFavorite(entry.id, !entry.favorite)
      .then(refresh)
      .catch((err: unknown) => setNotice(errorText(err)));
  };

  const exportCsv = () => {
    dictionaryApi
      .exportCsv()
      .then((csv) => {
        const url = URL.createObjectURL(new Blob([csv], { type: "text/csv;charset=utf-8" }));
        const link = document.createElement("a");
        link.href = url;
        link.download = "vyrp-dicionario.csv";
        link.click();
        URL.revokeObjectURL(url);
      })
      .catch((err: unknown) => setNotice(errorText(err)));
  };

  const importCsv = (file: File) => {
    file
      .text()
      .then((csv) => dictionaryApi.importCsv(csv))
      .then((report) => {
        setNotice(t("dict.imported", { summary: importSummary(report) }));
        refresh();
      })
      .catch((err: unknown) => setNotice(errorText(err)));
  };

  const startEdit = (next: DictionaryDraft) => {
    setDraft(next);
    setFormError(null);
  };

  return (
    <section className="dictionary" aria-label={t("dict.aria")}>
      <div className="toolbar">
        <input
          type="search"
          placeholder={t("dict.search")}
          aria-label={t("dict.searchLabel")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <button type="button" className="primary" onClick={() => startEdit(emptyDraft())}>
          {t("dict.add")}
        </button>
        <button type="button" onClick={() => fileInput.current?.click()}>
          {t("dict.import")}
        </button>
        <button type="button" onClick={exportCsv}>
          {t("dict.export")}
        </button>
        <input
          ref={fileInput}
          type="file"
          accept=".csv,text/csv"
          hidden
          onChange={(e) => {
            const file = e.target.files?.[0];
            if (file) importCsv(file);
            e.target.value = "";
          }}
        />
      </div>

      {notice && (
        <p className="notice" role="status" onClick={() => setNotice(null)}>
          {notice}
        </p>
      )}

      {draft && (
        <form
          className="card entry-form"
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
        >
          <h2>{draft.id === null ? t("dict.form.new") : t("dict.form.edit")}</h2>
          <label>
            {t("dict.form.phrase")}
            <input
              autoFocus
              value={draft.phrase}
              onChange={(e) => setDraft({ ...draft, phrase: e.target.value })}
            />
          </label>
          <label>
            {draft.is_snippet ? t("dict.form.expansion") : t("dict.form.replacement")}
            <textarea
              rows={3}
              value={draft.replacement ?? ""}
              onChange={(e) => setDraft({ ...draft, replacement: e.target.value })}
            />
          </label>
          <label className="check">
            <input
              type="checkbox"
              checked={draft.is_snippet}
              onChange={(e) => setDraft({ ...draft, is_snippet: e.target.checked })}
            />
            {t("dict.form.isSnippet")}
          </label>
          {formError && (
            <p className="error" role="alert">
              {formError}
            </p>
          )}
          <div className="actions">
            <button type="submit" className="primary">
              {t("common.save")}
            </button>
            <button
              type="button"
              onClick={() => {
                setDraft(null);
                setFormError(null);
              }}
            >
              {t("common.cancel")}
            </button>
          </div>
        </form>
      )}

      {entries.length === 0 ? (
        <p className="empty">
          {search.trim() === ""
            ? t("dict.empty")
            : t("dict.emptySearch")}
        </p>
      ) : (
        <ul className="entries">
          {entries.map((entry) => (
            <li key={entry.id}>
              <button
                type="button"
                className={entry.favorite ? "star on" : "star"}
                aria-label={entry.favorite ? t("dict.fav.on") : t("dict.fav.off")}
                aria-pressed={entry.favorite}
                onClick={() => toggleFavorite(entry)}
              >
                {entry.favorite ? "★" : "☆"}
              </button>
              <div className="entry-text">
                <strong>{entry.phrase}</strong>
                {entry.replacement && <span className="arrow"> → {entry.replacement}</span>}
              </div>
              <span className={entry.is_snippet ? "badge snippet" : "badge"}>{kindLabel(entry)}</span>
              {confirmingId === entry.id ? (
                <span className="confirm">
                  {t("dict.confirmDelete")}
                  <button type="button" className="danger" onClick={() => remove(entry.id)}>
                    {t("common.yes")}
                  </button>
                  <button type="button" onClick={() => setConfirmingId(null)}>
                    {t("common.no")}
                  </button>
                </span>
              ) : (
                <span className="row-actions">
                  <button type="button" onClick={() => startEdit(draftFromEntry(entry))}>
                    {t("dict.edit")}
                  </button>
                  <button type="button" onClick={() => setConfirmingId(entry.id)}>
                    {t("common.delete")}
                  </button>
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
