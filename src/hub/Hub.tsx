import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Dictionary from "./Dictionary";

interface AppInfo {
  version: string;
  log_dir: string;
}

type Tab = "home" | "dictionary";

const TABS: { id: Tab; label: string }[] = [
  { id: "home", label: "Início" },
  { id: "dictionary", label: "Dicionário" },
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
            {label}
          </button>
        ))}
      </nav>
      {tab === "home" ? <Home /> : <Dictionary />}
    </main>
  );
}
