import { useCallback, useEffect, useState } from "react";
import { describeAction, parsePhrases } from "./format";
import { t } from "./i18n";
import type { StringKey } from "./i18n";
import { settingsApi } from "./settingsApi";
import type { ApiProvider, HistoryRetention, VoiceCommand } from "./settingsApi";
import { errorText } from "./useSettings";
import type { SettingsState } from "./useSettings";

const RETENTION: { value: HistoryRetention; label: StringKey }[] = [
  { value: "keep", label: "advanced.retention.keep" },
  { value: "day", label: "advanced.retention.day" },
  { value: "never", label: "advanced.retention.never" },
];

const PROVIDERS: { id: ApiProvider; name: string }[] = [
  { id: "groq", name: "Groq" },
  { id: "open_ai", name: "OpenAI" },
  { id: "anthropic", name: "Anthropic" },
];

function PhrasesField({ command, onCommit }: { command: VoiceCommand; onCommit: (phrases: string[]) => void }) {
  const shown = command.phrases.join(", ");
  const [draft, setDraft] = useState(shown);
  const commit = () => {
    const phrases = parsePhrases(draft);
    if (phrases.join(", ") !== shown) onCommit(phrases);
    else setDraft(shown);
  };
  return (
    <input
      aria-label={describeAction(command.action)}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
      }}
    />
  );
}

function ApiKeyRow({ provider, name }: { provider: ApiProvider; name: string }) {
  const [present, setPresent] = useState<boolean | null>(null);
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    settingsApi
      .hasKey(provider)
      .then(setPresent)
      .catch((err: unknown) => setError(errorText(err)));
  }, [provider]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const save = () => {
    settingsApi
      .setKey(provider, value)
      .then(() => {
        // A chave nunca fica na tela nem no estado depois de salva.
        setValue("");
        setError(null);
        refresh();
      })
      .catch((err: unknown) => setError(errorText(err)));
  };
  const clear = () => {
    settingsApi
      .clearKey(provider)
      .then(() => {
        setError(null);
        refresh();
      })
      .catch((err: unknown) => setError(errorText(err)));
  };

  return (
    <div className="key-row">
      <strong className="key-name">{name}</strong>
      <span className={present ? "badge snippet" : "badge"}>
        {present === null ? "…" : present ? t("advanced.keys.set") : t("advanced.keys.unset")}
      </span>
      <input
        type="password"
        autoComplete="off"
        aria-label={`Chave ${name}`}
        placeholder={t("advanced.keys.placeholder")}
        value={value}
        onChange={(e) => setValue(e.target.value)}
      />
      <button type="button" className="primary" disabled={value.trim() === ""} onClick={save}>
        {t("common.save")}
      </button>
      <button type="button" disabled={!present} onClick={clear}>
        {t("common.remove")}
      </button>
      {error && (
        <p className="error key-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

export default function Advanced({ settings, error, update }: SettingsState) {
  if (!settings) return <p className="hint">{error ?? t("common.loading")}</p>;

  const setPhrases = (index: number, phrases: string[]) => {
    const next = settings.voice_commands.map((c, i) => (i === index ? { ...c, phrases } : c));
    update({ voice_commands: next });
  };
  const restoreCommands = () => {
    settingsApi
      .defaults()
      .then((defaults) => update({ voice_commands: defaults.voice_commands }))
      .catch(() => undefined);
  };

  return (
    <div className="settings">
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <section className="card">
        <h2>{t("advanced.retention.title")}</h2>
        <div className="radios" role="radiogroup" aria-label={t("advanced.retention.title")}>
          {RETENTION.map(({ value, label }) => (
            <label key={value} className="check">
              <input
                type="radio"
                name="retention"
                checked={settings.history_retention === value}
                onChange={() => update({ history_retention: value })}
              />
              {t(label)}
            </label>
          ))}
        </div>
        <p className="hint small">{t("advanced.retention.hint")}</p>
      </section>

      <section className="card">
        <h2>{t("advanced.voice.title")}</h2>
        <p className="hint small">{t("advanced.voice.hint")}</p>
        {settings.voice_commands.length === 0 ? (
          <p className="hint">{t("advanced.voice.empty")}</p>
        ) : (
          <ul className="voice-commands">
            {settings.voice_commands.map((command, index) => (
              <li key={describeAction(command.action) + index}>
                <span className="voice-action">{describeAction(command.action)}</span>
                <PhrasesField command={command} onCommit={(p) => setPhrases(index, p)} />
              </li>
            ))}
          </ul>
        )}
        <div className="actions">
          <button type="button" onClick={restoreCommands}>
            {t("common.restoreDefault")}
          </button>
          <button
            type="button"
            disabled={settings.voice_commands.length === 0}
            onClick={() => update({ voice_commands: [] })}
          >
            {t("advanced.voice.off")}
          </button>
        </div>
      </section>

      <section className="card">
        <h2>{t("advanced.keys.title")}</h2>
        <p className="hint small">{t("advanced.keys.hint")}</p>
        {PROVIDERS.map((p) => (
          <ApiKeyRow key={p.id} provider={p.id} name={p.name} />
        ))}
      </section>
    </div>
  );
}
