import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import ModelPicker from "./ModelPicker";
import ShortcutRecorder from "./ShortcutRecorder";
import { t, type StringKey } from "./i18n";
import { chordLabel } from "./keys";
import { modelName } from "./modelsApi";
import {
  STEPS,
  canAdvance,
  levelPercent,
  looksSilent,
  nextStep,
  previousStep,
  recommendedModel,
  stepPosition,
  trackPeak,
  usableModel,
  type Step,
} from "./onboardingState";
import { audioApi, settingsApi, type DictationTest } from "./settingsApi";
import { EVENT_LEVEL, EVENT_MONITOR, type AudioLevel } from "../shared/dictation";
import { errorText, type SettingsState } from "./useSettings";
import { useModels, type ModelsState } from "./useModels";

const STEP_TITLES: Record<Step, StringKey> = {
  welcome: "onb.welcome.title",
  microphone: "onb.mic.title",
  model: "onb.model.title",
  shortcut: "onb.shortcut.title",
  test: "onb.test.title",
  done: "onb.done.title",
};

/** Barra de nível ao vivo (evento `dictation://level`). Monte só enquanto estiver ouvindo. */
function LiveMeter({ warnWhenSilent, restartable }: { warnWhenSilent: boolean; restartable?: boolean }) {
  const [level, setLevel] = useState(0);
  // Um ditado (atalho real) pode tomar o microfone: o medidor se apaga até ele devolver.
  const [taken, setTaken] = useState(false);
  const [peak, setPeak] = useState(0);
  const [listenedMs, setListenedMs] = useState(0);

  useEffect(() => {
    const off = listen<AudioLevel>(EVENT_LEVEL, (e) => {
      setLevel(e.payload.rms);
      setPeak((p) => trackPeak(p, e.payload.rms));
    });
    const offMonitor = listen<{ active: boolean }>(EVENT_MONITOR, (e) => {
      if (!e.payload.active) {
        setTaken(true);
        setLevel(0);
      }
    });
    const timer = window.setInterval(() => setListenedMs((ms) => ms + 500), 500);
    return () => {
      void off.then((fn) => fn());
      void offMonitor.then((fn) => fn());
      window.clearInterval(timer);
    };
  }, []);

  const retake = () => {
    audioApi
      .monitorStart()
      .then(() => setTaken(false))
      .catch(() => undefined);
  };

  return (
    <div className="meter-box">
      <div className="meter" role="meter" aria-valuenow={taken ? 0 : levelPercent(level)} aria-valuemin={0} aria-valuemax={100}>
        <span style={{ width: `${taken ? 0 : levelPercent(level)}%` }} />
      </div>
      {taken && (
        <p className="hint small" role="status">
          {t("onb.mic.taken")}{" "}
          {restartable && (
            <button type="button" className="link" onClick={retake}>
              {t("onb.mic.retake")}
            </button>
          )}
        </p>
      )}
      {warnWhenSilent && !taken && looksSilent(peak, listenedMs) && (
        <p className="error small" role="alert">
          {t("onb.mic.silent")}
        </p>
      )}
    </div>
  );
}

function WelcomeStep() {
  return (
    <>
      <p>{t("onb.welcome.lead")}</p>
      <ul className="bullets">
        <li>{t("onb.welcome.b1")}</li>
        <li>{t("onb.welcome.b2")}</li>
        <li>{t("onb.welcome.b3")}</li>
      </ul>
    </>
  );
}

function MicStep({ settings, update }: SettingsState) {
  const [mics, setMics] = useState<string[]>([]);

  useEffect(() => {
    settingsApi
      .microphones()
      .then(setMics)
      .catch(() => setMics([]));
  }, []);

  // O medidor precisa do microfone aberto: ouve só enquanto esta etapa está na tela.
  useEffect(() => {
    void audioApi.monitorStart().catch(() => undefined);
    return () => {
      void audioApi.monitorStop().catch(() => undefined);
    };
  }, []);

  const chosen = settings?.microphone ?? null;
  return (
    <>
      <p>{t("onb.mic.lead")}</p>
      <label className="field">
        <span className="field-label">{t("general.mic.label")}</span>
        <select
          value={chosen ?? ""}
          disabled={!settings}
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
      <p className="hint small">{t("onb.mic.speak")}</p>
      <LiveMeter warnWhenSilent restartable />
      <p className="hint small">{t("onb.mic.permission")}</p>
    </>
  );
}

function ModelStep({ models }: { models: ModelsState }) {
  const suggestion = recommendedModel(models.models);
  return (
    <>
      <p>{t("onb.model.lead")}</p>
      {suggestion && !usableModel(models.models) && (
        <p className="hint small">{t("onb.model.suggest", { name: modelName(suggestion.id) })}</p>
      )}
      <ModelPicker {...models} />
      {!usableModel(models.models) && <p className="hint small">{t("onb.model.required")}</p>}
    </>
  );
}

function ShortcutStep({ settings, update }: SettingsState) {
  if (!settings) return <p className="hint">{t("common.loading")}</p>;
  const g = settings.gestures;
  return (
    <>
      <p>{t("onb.shortcut.lead", { keys: chordLabel(g.ptt) })}</p>
      <ShortcutRecorder
        label={t("general.keys.ptt")}
        value={g.ptt}
        onChange={(ptt) => update({ gestures: { ...g, ptt } })}
      />
      <ShortcutRecorder
        label={t("general.keys.lock")}
        value={g.lock}
        emptyText={t("general.keys.lockNone")}
        onChange={(lock) => update({ gestures: { ...g, lock } })}
        onClear={() => update({ gestures: { ...g, lock: null } })}
      />
      <p className="hint small">{t("onb.shortcut.tip")}</p>
    </>
  );
}

type TestPhase =
  | { kind: "idle" }
  | { kind: "recording" }
  | { kind: "working" }
  | { kind: "result"; result: DictationTest }
  | { kind: "error"; message: string };

/**
 * Ditado de teste: grava e transcreve pelos commands do hub. Quando o orquestrador (V11)
 * existir, o atalho real passa a funcionar aqui também; este botão continua útil.
 */
function TestStep() {
  const [phase, setPhase] = useState<TestPhase>({ kind: "idle" });

  const start = () => {
    setPhase({ kind: "recording" });
    audioApi.testStart().catch((err: unknown) => setPhase({ kind: "error", message: errorText(err) }));
  };
  const stop = () => {
    setPhase({ kind: "working" });
    audioApi
      .testStop()
      .then((result) => setPhase({ kind: "result", result }))
      .catch((err: unknown) => setPhase({ kind: "error", message: errorText(err) }));
  };

  // Sair da etapa gravando descarta a gravação.
  useEffect(
    () => () => {
      void audioApi.monitorStop().catch(() => undefined);
    },
    [],
  );

  return (
    <>
      <p>{t("onb.test.lead")}</p>
      <div className="test-controls">
        {(phase.kind === "idle" || phase.kind === "result" || phase.kind === "error") && (
          <button type="button" className="primary" onClick={start}>
            {phase.kind === "idle" ? t("onb.test.record") : t("onb.test.again")}
          </button>
        )}
        {phase.kind === "recording" && (
          <button type="button" className="primary" onClick={stop}>
            {t("onb.test.stop")}
          </button>
        )}
        {phase.kind === "working" && <span className="hint">{t("onb.test.working")}</span>}
      </div>
      {phase.kind === "recording" && <LiveMeter warnWhenSilent={false} />}
      {phase.kind === "result" && (
        <div className="test-result">
          <p className="result-text">{phase.result.text === "" ? t("onb.test.empty") : phase.result.text}</p>
          <p className="hint small">
            {t("onb.test.stats", {
              audio: (phase.result.audio_ms / 1000).toFixed(1).replace(".", ","),
              decode: (phase.result.decode_ms / 1000).toFixed(1).replace(".", ","),
            })}
          </p>
        </div>
      )}
      {phase.kind === "error" && (
        <p className="error" role="alert">
          {t("onb.test.failed", { error: phase.message })}
        </p>
      )}
      <p className="hint small">{t("onb.test.skip")}</p>
    </>
  );
}

function DoneStep({ settings, models }: SettingsState & { models: ModelsState }) {
  const model = usableModel(models.models);
  return (
    <>
      <p>{t("onb.done.lead")}</p>
      <dl className="summary">
        <dt>{t("onb.done.mic")}</dt>
        <dd>{settings?.microphone ?? t("general.mic.default")}</dd>
        <dt>{t("onb.done.model")}</dt>
        <dd>{model ? modelName(model.id) : t("onb.done.noModel")}</dd>
        <dt>{t("onb.done.shortcut")}</dt>
        <dd>{settings ? chordLabel(settings.gestures.ptt) : "…"}</dd>
      </dl>
      <p className="hint small">{t("onb.done.later")}</p>
    </>
  );
}

interface Props {
  settingsState: SettingsState;
  onFinish: () => void;
}

export default function Onboarding({ settingsState, onFinish }: Props) {
  const [step, setStep] = useState<Step>("welcome");
  const models = useModels();
  const { index, total } = stepPosition(step);
  const hasUsableModel = usableModel(models.models) !== undefined;
  const last = step === "done";

  let body: React.ReactNode;
  switch (step) {
    case "welcome":
      body = <WelcomeStep />;
      break;
    case "microphone":
      body = <MicStep {...settingsState} />;
      break;
    case "model":
      body = <ModelStep models={models} />;
      break;
    case "shortcut":
      body = <ShortcutStep {...settingsState} />;
      break;
    case "test":
      body = <TestStep />;
      break;
    case "done":
      body = <DoneStep {...settingsState} models={models} />;
      break;
  }

  return (
    <main className="hub onboarding">
      <header>
        <h1>Vyrp</h1>
        <p className="tagline">{t("onb.tagline")}</p>
      </header>
      <ol className="stepper" aria-label={t("onb.progress", { index, total })}>
        {STEPS.map((s) => (
          <li key={s} className={s === step ? "on" : STEPS.indexOf(s) < STEPS.indexOf(step) ? "past" : ""}>
            <span />
          </li>
        ))}
      </ol>
      <section className="card onboarding-card">
        <p className="hint small">{t("onb.progress", { index, total })}</p>
        <h2>{t(STEP_TITLES[step])}</h2>
        {body}
      </section>
      <footer className="onboarding-nav">
        <button type="button" onClick={() => setStep(previousStep(step))} disabled={step === "welcome"}>
          {t("onb.back")}
        </button>
        <span className="spacer" />
        {!last && (
          <button type="button" className="link" onClick={onFinish}>
            {t("onb.skipAll")}
          </button>
        )}
        {last ? (
          <button type="button" className="primary" onClick={onFinish}>
            {t("onb.finish")}
          </button>
        ) : (
          <button
            type="button"
            className="primary"
            disabled={!canAdvance(step, { hasUsableModel })}
            onClick={() => setStep(nextStep(step))}
          >
            {step === "welcome" ? t("onb.start") : t("onb.next")}
          </button>
        )}
      </footer>
    </main>
  );
}
