import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";
import {
  DEFAULT_APPS,
  DEFAULT_DOMAINS,
  addEntry,
  removeEntry,
  type EntryKind,
} from "./privacyRules";
import { errorText, type SettingsState } from "./useSettings";

function ListEditor({
  kind,
  items,
  onChange,
}: {
  kind: EntryKind;
  items: string[];
  onChange: (next: string[]) => void;
}) {
  const [draft, setDraft] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  const add = () => {
    const result = addEntry(items, kind, draft);
    if ("error" in result) {
      setProblem(result.error === "duplicate" ? t("priv.duplicate") : t("priv.empty"));
      return;
    }
    setProblem(null);
    setDraft("");
    onChange(result.list);
  };

  return (
    <div className="field">
      <span className="field-label">{t(kind === "app" ? "priv.apps.title" : "priv.domains.title")}</span>
      <div className="recorder">
        <input
          value={draft}
          placeholder={t(kind === "app" ? "priv.apps.placeholder" : "priv.domains.placeholder")}
          aria-label={t(kind === "app" ? "priv.apps.title" : "priv.domains.title")}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") add();
          }}
        />
        <button type="button" onClick={add} disabled={draft.trim() === ""}>
          {t("priv.add")}
        </button>
      </div>
      {problem && (
        <p className="error small" role="alert">
          {problem}
        </p>
      )}
      {items.length > 0 && (
        <ul className="chips-list">
          {items.map((item) => (
            <li key={item}>
              <span className="mono">{item}</span>
              <button type="button" aria-label={t("priv.remove", { item })} onClick={() => onChange(removeEntry(items, item))}>
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Aba Avançado › Privacidade: lista de apps/sites bloqueados e áudio no histórico. */
export default function PrivacySettings({ settings, update }: SettingsState) {
  const [notice, setNotice] = useState<string | null>(null);
  if (!settings) return null;
  const privacy = settings.privacy;
  const setPrivacy = (patch: Partial<typeof privacy>) => update({ privacy: { ...privacy, ...patch } });

  const clearAudio = () => {
    invoke<number>("history_clear_audio")
      .then((n) => setNotice(t("priv.audio.cleared", { n })))
      .catch((err: unknown) => setNotice(errorText(err)));
  };

  return (
    <section className="card">
      <h2>{t("priv.title")}</h2>
      <p className="hint small">{t("priv.lead")}</p>

      <label className="check">
        <input
          type="checkbox"
          checked={privacy.block_defaults}
          onChange={(e) => setPrivacy({ block_defaults: e.target.checked })}
        />
        {t("priv.defaults")}
      </label>
      <p className="hint small">
        {t("priv.defaults.list", { apps: DEFAULT_APPS.join(", "), sites: DEFAULT_DOMAINS.join(", ") })}
      </p>

      <ListEditor kind="app" items={privacy.apps} onChange={(apps) => setPrivacy({ apps })} />
      <ListEditor kind="domain" items={privacy.domains} onChange={(domains) => setPrivacy({ domains })} />
      <p className="hint small">{t("priv.behavior")}</p>

      <label className="check">
        <input
          type="checkbox"
          checked={privacy.save_audio}
          onChange={(e) => setPrivacy({ save_audio: e.target.checked })}
        />
        {t("priv.audio")}
      </label>
      <p className="hint small">{t("priv.audio.hint")}</p>
      <div className="actions">
        <button type="button" onClick={clearAudio}>
          {t("priv.audio.clear")}
        </button>
      </div>
      {notice && (
        <p className="hint small" role="status">
          {notice}
        </p>
      )}
    </section>
  );
}
