import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Advanced from "./Advanced";
import Transforms from "./Transforms";
import Dictionary from "./Dictionary";
import Diagnostics from "./Diagnostics";
import General from "./General";
import History from "./History";
import Onboarding from "./Onboarding";
import Providers from "./Providers";
import {
  clearLegacyOnboarding,
  readLegacyOnboardingDone,
  shouldMigrateLegacyFlag,
} from "./onboardingState";
import { t } from "./i18n";
import { useUiLanguage } from "../shared/uiLanguage";
import { useSettings } from "./useSettings";

interface AppInfo {
  version: string;
  log_dir: string;
}

type Tab = "home" | "general" | "providers" | "history" | "dictionary" | "diagnostics" | "advanced" | "transforms";

const TABS: { id: Tab; label: () => string }[] = [
  { id: "home", label: () => t("tab.home") },
  { id: "general", label: () => t("tab.general") },
  { id: "providers", label: () => t("tab.providers") },
  { id: "history", label: () => t("tab.history") },
  { id: "dictionary", label: () => t("tab.dictionary") },
  { id: "transforms", label: () => t("transform.title") },
  { id: "diagnostics", label: () => t("tab.diagnostics") },
  { id: "advanced", label: () => t("tab.advanced") },
];

function Home({ onRedoOnboarding }: { onRedoOnboarding: () => void }) {
  const [info, setInfo] = useState<AppInfo | null>(null);

  useEffect(() => {
    invoke<AppInfo>("app_info").then(setInfo).catch(() => setInfo(null));
  }, []);

  return (
    <>
      <section className="card">
        <h2>{t("home.title")}</h2>
        <p>{t("home.skeleton")}</p>
        <dl>
          <dt>{t("home.version")}</dt>
          <dd>{info?.version ?? "…"}</dd>
          <dt>{t("home.logs")}</dt>
          <dd className="mono">{info?.log_dir ?? "…"}</dd>
        </dl>
      </section>
      <button type="button" onClick={onRedoOnboarding}>
        {t("onb.redo")}
      </button>
      <p className="hint">{t("home.hint")}</p>
    </>
  );
}

export default function Hub() {
  const [tab, setTab] = useState<Tab>("home");
  const language = useUiLanguage();
  const settingsState = useSettings();
  const { settings, update, error, retry } = settingsState;

  // Quem concluiu o onboarding antes desta versão tinha a flag no localStorage: passa uma vez
  // para o Settings.
  useEffect(() => {
    if (settings && shouldMigrateLegacyFlag(settings.onboarding_done, readLegacyOnboardingDone())) {
      clearLegacyOnboarding();
      update({ onboarding_done: true });
    }
  }, [settings, update]);

  const setOnboardingDone = (done: boolean) => {
    update({ onboarding_done: done });
    setTab("home");
  };

  if (settings === null) {
    return (
      <main className="hub">
        {error ? (
          <>
            <p className="error" role="alert">
              {error}
            </p>
            <button type="button" className="primary" onClick={retry}>
              {t("common.retry")}
            </button>
          </>
        ) : (
          <p className="hint">{t("common.loading")}</p>
        )}
      </main>
    );
  }

  if (!settings.onboarding_done) {
    return <Onboarding settingsState={settingsState} onFinish={() => setOnboardingDone(true)} />;
  }

  return (
    <main className="hub" lang={language}>
      <header>
        <h1>Vyrp</h1>
        <p className="tagline">{t("app.tagline")}</p>
      </header>
      <nav className="tabs" role="tablist">
        {TABS.map(({ id, label }) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            className={tab === id ? "tab on" : "tab"}
            onClick={() => setTab(id)}
          >
            {label()}
          </button>
        ))}
      </nav>
      {tab === "home" && <Home onRedoOnboarding={() => setOnboardingDone(false)} />}
      {tab === "general" && <General {...settingsState} />}
      {tab === "providers" && <Providers {...settingsState} />}
      {tab === "history" && <History retention={settingsState.settings?.history_retention ?? null} />}
      {tab === "dictionary" && <Dictionary />}
      {tab === "transforms" && <Transforms {...settingsState} />}
      {tab === "diagnostics" && <Diagnostics />}
      {tab === "advanced" && <Advanced {...settingsState} />}
    </main>
  );
}
