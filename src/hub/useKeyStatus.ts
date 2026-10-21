import { useCallback, useEffect, useState } from "react";
import type { KeyStatus } from "./providerRules";
import { settingsApi, type ApiProvider } from "./settingsApi";

const ALL: ApiProvider[] = ["groq", "open_ai", "anthropic"];

/** Quais chaves de API existem no Credential Manager (só presença; o valor nunca vem). */
export function useKeyStatus(): { status: KeyStatus; refresh: () => void } {
  const [status, setStatus] = useState<KeyStatus>({});

  const refresh = useCallback(() => {
    for (const provider of ALL) {
      settingsApi
        .hasKey(provider)
        .then((present) => setStatus((prev) => ({ ...prev, [provider]: present })))
        .catch(() => setStatus((prev) => ({ ...prev, [provider]: null })));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return { status, refresh };
}
