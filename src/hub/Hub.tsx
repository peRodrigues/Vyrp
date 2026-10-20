import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface AppInfo {
  version: string;
  log_dir: string;
}

export default function Hub() {
  const [info, setInfo] = useState<AppInfo | null>(null);

  useEffect(() => {
    invoke<AppInfo>("app_info").then(setInfo).catch(() => setInfo(null));
  }, []);

  return (
    <main className="hub">
      <header>
        <h1>Vyrp</h1>
        <p className="tagline">Segure o atalho, fale, solte: o texto aparece onde está o cursor.</p>
      </header>
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
    </main>
  );
}
