import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { SETTINGS_CHANGED, settingsApi } from "./settingsApi";
import type { Settings } from "./settingsApi";
import { t } from "./i18n";

export function errorText(err: unknown): string {
  return typeof err === "string" ? err : err instanceof Error ? err.message : "erro desconhecido";
}

export interface SettingsState {
  settings: Settings | null;
  error: string | null;
  /** Aplica um pedaço e salva; o backend valida e devolve o valor final. */
  update: (patch: Partial<Settings>) => void;
}

/** Settings do backend, atualizados em tempo real pelo evento `settings://changed`. */
export function useSettings(): SettingsState {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    settingsApi
      .get()
      .then(setSettings)
      .catch((err: unknown) => setError(errorText(err)));
    const unlisten = listen<Settings>(SETTINGS_CHANGED, (event) => setSettings(event.payload));
    return () => {
      void unlisten.then((off) => off());
    };
  }, []);

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

  return { settings, error, update };
}
