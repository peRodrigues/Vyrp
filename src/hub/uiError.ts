import { hasKey, t } from "./i18n";

/**
 * Erro que vem do Rust: um código estável + parâmetros (src-tauri/src/core/ui_error.rs).
 * A UI traduz pela chave `uierr.<code>`; `raw` é só texto (erro de SO/biblioteca, sem tradução).
 */
export interface UiError {
  code: string;
  params?: Record<string, string>;
}

const RAW = "raw";

export function isUiError(value: unknown): value is UiError {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as { code?: unknown }).code === "string"
  );
}

export const rawError = (text: string): UiError => ({ code: RAW, params: { text } });

/** Qualquer coisa que um `invoke` possa rejeitar, no formato de erro da tela. */
export function toUiError(err: unknown): UiError {
  if (isUiError(err)) return err;
  if (typeof err === "string") return rawError(err);
  if (err instanceof Error) return rawError(err.message);
  return rawError(t("common.unknownError"));
}

/** O texto do erro no idioma atual. Código sem tradução cai no texto original (ou no código). */
export function uiErrorText(err: unknown): string {
  const e = toUiError(err);
  const key = `uierr.${e.code}`;
  if (e.code !== RAW && hasKey(key)) return t(key, e.params);
  return e.params?.text ?? e.params?.detail ?? e.code;
}
