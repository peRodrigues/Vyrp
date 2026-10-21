import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { t, type StringKey } from "./i18n";
import {
  EVENT_LEARNED,
  LEARN_MODES,
  learnApi,
  toLearnMode,
  type LearnedEvent,
  type LearnMode,
  type Suggestion,
} from "./learnApi";
import { errorText, useSettings } from "./useSettings";

const MODE_LABEL: Record<LearnMode, StringKey> = {
  off: "learn.mode.off",
  suggest: "learn.mode.suggest",
  auto: "learn.mode.auto",
};

interface Props {
  /** O dicionário mudou (sugestão aceita, termo aprendido ou desfeito): refaz a lista. */
  onChanged: () => void;
}

/** Auto-aprendizado do dicionário: modo, sugestões pendentes e o aviso do modo automático. */
export default function Learning({ onChanged }: Props) {
  const { settings, update } = useSettings();
  const [suggestions, setSuggestions] = useState<Suggestion[]>([]);
  const [learned, setLearned] = useState<LearnedEvent | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const refresh = useCallback(() => {
    learnApi
      .suggestions()
      .then(setSuggestions)
      .catch((err: unknown) => setNotice(errorText(err)));
  }, []);

  useEffect(() => {
    refresh();
    const sub = listen<LearnedEvent>(EVENT_LEARNED, (e) => {
      if (e.payload.mode === "auto") {
        setLearned(e.payload);
        onChanged();
      } else {
        refresh();
      }
    });
    return () => {
      void sub.then((off) => off());
    };
  }, [refresh, onChanged]);

  const mode = toLearnMode(settings?.dictionary_learning);

  const accept = (id: number) => {
    learnApi
      .accept(id)
      .then(() => {
        setNotice(null);
        refresh();
        onChanged();
      })
      .catch((err: unknown) => {
        setNotice(errorText(err));
        refresh();
      });
  };

  const dismiss = (id: number) => {
    learnApi
      .dismiss(id)
      .then(refresh)
      .catch((err: unknown) => setNotice(errorText(err)));
  };

  const undo = (entryId: number) => {
    learnApi
      .undo(entryId)
      .then(() => {
        setLearned(null);
        setNotice(null);
        onChanged();
      })
      .catch((err: unknown) => setNotice(errorText(err)));
  };

  return (
    <section className="card learning" aria-label={t("learn.title")}>
      <h2>{t("learn.title")}</h2>
      <p className="hint small">{t("learn.lead")}</p>
      <label className="field">
        <span className="field-label">{t("learn.mode.label")}</span>
        <select
          value={mode}
          disabled={!settings}
          onChange={(e) => update({ dictionary_learning: toLearnMode(e.target.value) })}
        >
          {LEARN_MODES.map((m) => (
            <option key={m} value={m}>
              {t(MODE_LABEL[m])}
            </option>
          ))}
        </select>
      </label>

      {learned && (
        <p className="notice" role="status">
          {t("learn.auto.notice", { phrase: learned.phrase, observed: learned.observed })}{" "}
          <button type="button" onClick={() => undo(learned.id)}>
            {t("learn.undo")}
          </button>
          <button type="button" onClick={() => setLearned(null)}>
            {t("common.close")}
          </button>
        </p>
      )}

      {mode === "suggest" && (
        <>
          <h3>{t("learn.suggestions.title")}</h3>
          {suggestions.length === 0 ? (
            <p className="hint small">{t("learn.suggestions.empty")}</p>
          ) : (
            <ul className="entries">
              {suggestions.map((s) => (
                <li key={s.id}>
                  <div className="entry-text">
                    <strong>{s.phrase}</strong>
                    <span className="arrow"> {t("learn.heardAs", { observed: s.observed })}</span>
                  </div>
                  <span className="row-actions">
                    <button type="button" className="primary" onClick={() => accept(s.id)}>
                      {t("learn.accept")}
                    </button>
                    <button type="button" onClick={() => dismiss(s.id)}>
                      {t("learn.dismiss")}
                    </button>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}

      {notice && (
        <p className="error small" role="alert">
          {notice}
        </p>
      )}
    </section>
  );
}
