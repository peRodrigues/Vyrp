import { useEffect, useState } from "react";
import ShortcutRecorder from "./ShortcutRecorder";
import ModelPicker from "./ModelPicker";
import { mergeRanking, moveItem } from "./format";
import { t, type UiLanguageSetting } from "./i18n";
import { chordLabel } from "./keys";
import { settingsApi } from "./settingsApi";
import { OVERLAY_PREF_KEY, hideForever, parsePref, readPref, writePref } from "../shared/overlayPrefs";
import type { GestureConfig } from "./settingsApi";
import { useModels } from "./useModels";
import type { SettingsState } from "./useSettings";

const UI_LANGUAGES: UiLanguageSetting[] = ["auto", "pt-BR", "en"];

const LANGUAGES: { code: string; name: string }[] = [
  { code: "pt", name: "Português" },
  { code: "en", name: "English" },
  { code: "es", name: "Español" },
  { code: "fr", name: "Français" },
  { code: "de", name: "Deutsch" },
  { code: "it", name: "Italiano" },
];

interface NumberFieldProps {
  label: string;
  value: number;
  min?: number;
  max?: number;
  onCommit: (n: number) => void;
}

/** O `key` recria o campo quando o valor salvo muda (ex.: o backend ajustou o número). */
function NumberField(props: NumberFieldProps) {
  return <NumberInput key={props.value} {...props} />;
}

function NumberInput(props: NumberFieldProps) {
  const [draft, setDraft] = useState(String(props.value));
  const commit = () => {
    const n = Number.parseInt(draft, 10);
    if (Number.isFinite(n) && n !== props.value) props.onCommit(n);
    else setDraft(String(props.value));
  };
  return (
    <label className="field">
      <span className="field-label">{props.label}</span>
      <input
        type="number"
        min={props.min}
        max={props.max}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
        }}
      />
    </label>
  );
}

function OverlayToggle() {
  const [pref, setPref] = useState(readPref);
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === OVERLAY_PREF_KEY || e.key === null) setPref(parsePref(e.newValue));
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);
  const visible = pref === null;
  return (
    <section className="card">
      <h2>{t("general.overlay.title")}</h2>
      <label className="check">
        <input
          type="checkbox"
          checked={visible}
          onChange={(e) => {
            const next = e.target.checked ? null : hideForever();
            writePref(next);
            setPref(next);
          }}
        />
        {t("general.overlay.show")}
      </label>
      <p className="hint small">{t("general.overlay.hint")}</p>
    </section>
  );
}

export default function General({ settings, error, update }: SettingsState) {
  const [mics, setMics] = useState<string[]>([]);
  const modelsState = useModels();

  useEffect(() => {
    settingsApi
      .microphones()
      .then(setMics)
      .catch(() => setMics([]));
  }, []);

  if (!settings) return <p className="hint">{error ?? t("common.loading")}</p>;

  const gestures = settings.gestures;
  const setGestures = (patch: Partial<GestureConfig>) => update({ gestures: { ...gestures, ...patch } });
  const chosen = settings.microphone;
  const ranking = mergeRanking(settings.mic_ranking, mics);
  const toggleLanguage = (code: string, on: boolean) => {
    const next = LANGUAGES.map((l) => l.code).filter((c) =>
      c === code ? on : settings.dictation_languages.includes(c),
    );
    update({ dictation_languages: next });
  };
  const restoreGestures = () => {
    settingsApi
      .defaults()
      .then((defaults) => update({ gestures: defaults.gestures }))
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
        <h2>{t("general.ui.title")}</h2>
        <label className="field">
          <span className="field-label">{t("general.ui.label")}</span>
          <select
            value={UI_LANGUAGES.includes(settings.ui_language as UiLanguageSetting) ? settings.ui_language : "auto"}
            onChange={(e) => update({ ui_language: e.target.value })}
          >
            <option value="auto">{t("general.ui.auto")}</option>
            <option value="pt-BR">Português (Brasil)</option>
            <option value="en">English</option>
          </select>
        </label>
        <p className="hint small">{t("general.ui.hint")}</p>
      </section>

      <section className="card">
        <h2>{t("general.mic.title")}</h2>
        <label className="field">
          <span className="field-label">{t("general.mic.label")}</span>
          <select
            value={chosen ?? ""}
            onChange={(e) => update({ microphone: e.target.value === "" ? null : e.target.value })}
          >
            <option value="">{t("general.mic.default")}</option>
            {chosen && !mics.includes(chosen) && (
              <option value={chosen}>{t("general.mic.absent", { name: chosen })}</option>
            )}
            {mics.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={settings.warm_mic}
            onChange={(e) => update({ warm_mic: e.target.checked })}
          />
          {t("general.mic.warm")}
        </label>
        <p className="hint small">{t("general.mic.warmHint")}</p>
        {settings.warm_mic && (
          <NumberField
            label={t("general.mic.preroll")}
            value={settings.preroll_ms}
            min={0}
            max={2000}
            onCommit={(n) => update({ preroll_ms: n })}
          />
        )}
        <div className="field">
          <span className="field-label">{t("general.mic.ranking")}</span>
          <p className="hint small">{t("general.mic.rankingHint")}</p>
          <ol className="ranking">
            {ranking.map((name, index) => (
              <li key={name}>
                <span className="entry-text">
                  {name}
                  {!mics.includes(name) && <span className="arrow"> · {t("general.mic.offline")}</span>}
                </span>
                <button
                  type="button"
                  aria-label={t("general.mic.up", { name })}
                  disabled={index === 0}
                  onClick={() => update({ mic_ranking: moveItem(ranking, index, index - 1) })}
                >
                  ↑
                </button>
                <button
                  type="button"
                  aria-label={t("general.mic.down", { name })}
                  disabled={index === ranking.length - 1}
                  onClick={() => update({ mic_ranking: moveItem(ranking, index, index + 1) })}
                >
                  ↓
                </button>
              </li>
            ))}
          </ol>
          {ranking.length === 0 && <p className="hint small">{t("general.mic.noDevices")}</p>}
        </div>
        <label className="check">
          <input
            type="checkbox"
            checked={settings.sounds}
            onChange={(e) => update({ sounds: e.target.checked })}
          />
          {t("general.sounds")}
        </label>
      </section>

      <section className="card">
        <h2>{t("general.lang.title")}</h2>
        <div className="chips">
          {LANGUAGES.map(({ code, name }) => (
            <label key={code} className="check chip">
              <input
                type="checkbox"
                checked={settings.dictation_languages.includes(code)}
                onChange={(e) => toggleLanguage(code, e.target.checked)}
              />
              {name}
            </label>
          ))}
        </div>
        <p className="hint small">{t("general.lang.hint")}</p>
      </section>

      <section className="card">
        <h2>{t("general.keys.title")}</h2>
        <ShortcutRecorder
          label={t("general.keys.ptt")}
          value={gestures.ptt}
          onChange={(ptt) => setGestures({ ptt })}
        />
        <ShortcutRecorder
          label={t("general.keys.lock")}
          value={gestures.lock}
          emptyText={t("general.keys.lockNone")}
          onChange={(lock) => setGestures({ lock })}
          onClear={() => setGestures({ lock: null })}
        />
        <div className="field">
          <span className="field-label">{t("general.keys.cancel")}</span>
          <kbd className="chord">{chordLabel([gestures.cancel])}</kbd>
        </div>
        <div className="row">
          <NumberField
            label={t("general.keys.minHold")}
            value={gestures.min_hold_ms}
            min={0}
            max={2000}
            onCommit={(n) => setGestures({ min_hold_ms: n })}
          />
          <NumberField
            label={t("general.keys.doubleTap")}
            value={gestures.double_tap_ms}
            min={100}
            max={1000}
            onCommit={(n) => setGestures({ double_tap_ms: n })}
          />
          <NumberField
            label={t("general.keys.debounce")}
            value={gestures.debounce_ms}
            min={0}
            max={2000}
            onCommit={(n) => setGestures({ debounce_ms: n })}
          />
        </div>
        <button type="button" onClick={restoreGestures}>
          {t("general.keys.restore")}
        </button>
      </section>

      <OverlayToggle />

      <section className="card">
        <h2>{t("general.model.title")}</h2>
        <ModelPicker {...modelsState} />
      </section>
    </div>
  );
}
