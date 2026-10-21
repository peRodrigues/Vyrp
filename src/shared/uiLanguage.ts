import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getLanguage, resolveLanguage, setLanguage, type Language } from "../hub/i18n";

const SETTINGS_CHANGED = "settings://changed";

interface WithLanguage {
  ui_language?: string;
}

/**
 * Idioma efetivo da interface desta janela (hub ou overlay): lê `ui_language` do Settings,
 * resolve "auto" pelo idioma do Windows e acompanha mudanças em tempo real. Chame uma vez no
 * topo de cada janela; o re-render da raiz refaz todos os `t()` abaixo.
 */
export function useUiLanguage(): Language {
  const [lang, setLang] = useState<Language>(getLanguage());

  useEffect(() => {
    const apply = (value: string | undefined) => {
      const next = resolveLanguage(value, window.navigator.language);
      setLanguage(next);
      setLang(next);
      document.documentElement.lang = next;
    };
    invoke<WithLanguage>("get_settings")
      .then((s) => apply(s.ui_language))
      .catch(() => undefined);
    const off = listen<WithLanguage>(SETTINGS_CHANGED, (e) => apply(e.payload.ui_language));
    return () => {
      void off.then((fn) => fn());
    };
  }, []);

  return lang;
}
