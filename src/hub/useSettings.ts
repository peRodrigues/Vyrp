import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { SETTINGS_CHANGED, settingsApi } from "./settingsApi";
import type { Settings } from "./settingsApi";
import { t } from "./i18n";
import { TimeoutError, withTimeout } from "./timeout";
import { uiErrorText } from "./uiError";

/** Texto de um erro (do Rust como `{code, params}`, ou string/Error) no idioma atual. */
export function errorText(err: unknown): string {
  if (err instanceof TimeoutError) return t("common.timeout");
  return uiErrorText(err);
}

export interface SettingsState {
  settings: Settings | null;
  error: string | null;
  /** Aplica um pedaço e salva; o backend valida e devolve o valor final. */
  update: (patch: Partial<Settings>) => void;
  /** Lê de novo os settings depois de um erro ou prazo esgotado. */
  retry?: () => void;
}

/** Settings do backend, atualizados em tempo real pelo evento `settings://changed`. */
export function useSettings(): SettingsState {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Com prazo: sem resposta do backend a tela mostra o erro e "Tentar de novo" em vez de
  // ficar em "Carregando…" para sempre.
  const load = useCallback(() => {
    withTimeout(settingsApi.get())
      .then((value) => {
        setSettings(value);
        setError(null);
      })
      .catch((err: unknown) => setError(t("common.loadError", { error: errorText(err) })));
  }, []);

  const retry = useCallback(() => {
    setError(null);
    load();
  }, [load]);

  useEffect(() => {
    load();
    const unlisten = listen<Settings>(SETTINGS_CHANGED, (event) => {
      setSettings(event.payload);
      setError(null);
    });
    return () => {
      void unlisten.then((off) => off());
    };
  }, [load]);

  const update = useCallback(
    (patch: Partial<Settings>) => {
      if (!settings) return;
      setError(null);
      settingsApi
        .set({ ...settings, ...patch })
        .then(setSettings)
        .catch((err: unknown) => setError(t("common.saveError", { error: errorText(err) })));
    },
    [settings],
  );

  return { settings, error, update, retry };
}
