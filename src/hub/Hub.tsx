import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Advanced from "./Advanced";
import Dictionary from "./Dictionary";
import General from "./General";
import History from "./History";
import { t } from "./i18n";
import { useSettings } from "./useSettings";

interface AppInfo {
  version: string;
  log_dir: string;
}

type Tab = "home" | "general" | "history" | "dictionary" | "advanced";

const TABS: { id: Tab; label: () => string }[] = [
  { id: "home", label: () => t("tab.home") },
  { id: "general", label: () => t("tab.general") },
  { id: "history", label: () => t("tab.history") },
  { id: "dictionary", label: () => t("tab.dictionary") },
  { id: "advanced", label: () => t("tab.advanced") },
];

function Home() {
  const [info, setInfo] = useState<AppInfo | null>(null);

  useEffect(() => {
    invoke<AppInfo>("app_info").then(setInfo).catch(() => setInfo(null));
  }, []);

  return (
    <>
      <section className="card">
        <h2>Estado</h2>
        <p>
          Esqueleto do app (card V2). Atalho, microfone e transcrição chegam nos próximos cards.
        </p>
        <dl>
          <dt>Versão</dt>
          <dd>{info?.version ?? "…"}</dd>
          <dt>Logs</dt>
          <dd className="mono">{info?.log_dir ?? "…"}</dd>
        </dl>
      </section>
      <p className="hint">Fechar esta janela mantém o Vyrp no tray. Para sair, use o menu do ícone.</p>
    </>
  );
}

export default function Hub() {
  const [tab, setTab] = useState<Tab>("home");
  const settingsState = useSettings();

  return (
    <main className="hub">
      <header>
        <h1>Vyrp</h1>
        <p className="tagline">Segure o atalho, fale, solte: o texto aparece onde está o cursor.</p>
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
      {tab === "home" && <Home />}
      {tab === "general" && <General {...settingsState} />}
      {tab === "history" && <History retention={settingsState.settings?.history_retention ?? null} />}
      {tab === "dictionary" && <Dictionary />}
      {tab === "advanced" && <Advanced {...settingsState} />}
    </main>
  );
}
