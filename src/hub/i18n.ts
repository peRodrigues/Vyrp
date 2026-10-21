// i18n do hub, do onboarding e do overlay: pt-BR (base) e en. Todo texto de tela passa por
// t(); chave nova entra nos dois dicionários (lang/pt-BR.ts e lang/en.ts) e o teste
// i18n.test.ts garante a paridade.

import { en } from "./lang/en";
import { ptBR } from "./lang/pt-BR";

export type StringKey = keyof typeof ptBR;
export type Language = "pt-BR" | "en";
/** Valor do Settings (`ui_language`): um idioma fixo ou "auto" (segue o Windows). */
export type UiLanguageSetting = "auto" | Language;

export const LANGUAGES: Language[] = ["pt-BR", "en"];

const dictionaries: Record<Language, Record<StringKey, string>> = { "pt-BR": ptBR, en };

/**
 * Idioma efetivo: o escolhido, ou, em "auto" (ou valor desconhecido), o do sistema, que o
 * WebView expõe em `navigator.language`. Português → pt-BR; qualquer outro → en.
 */
export function resolveLanguage(setting: string | null | undefined, system: string | null | undefined): Language {
  if (setting === "pt-BR" || setting === "en") return setting;
  return (system ?? "").toLowerCase().startsWith("pt") ? "pt-BR" : "en";
}

function initialLanguage(): Language {
  // Sem janela (testes em Node) o padrão é o idioma-base, para os testes serem determinísticos.
  if (typeof window === "undefined") return "pt-BR";
  return resolveLanguage("auto", window.navigator?.language);
}

let current: Language = initialLanguage();

export function setLanguage(lang: Language): void {
  current = lang;
}

export function getLanguage(): Language {
  return current;
}

/** A chave existe? (Para códigos que chegam de fora, como códigos de erro do backend.) */
export function hasKey(key: string): key is StringKey {
  return key in ptBR;
}

/** Separador decimal do idioma atual: "1,5" em pt-BR, "1.5" em en. */
export function decimal(value: string): string {
  return current === "pt-BR" ? value.replace(".", ",") : value;
}

/** Texto da tela; `{nome}` é trocado pelo valor de `vars.nome`. */
export function t(key: StringKey, vars?: Record<string, string | number>): string {
  const text: string = dictionaries[current][key] ?? ptBR[key];
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in vars ? String(vars[name]) : whole,
  );
}
