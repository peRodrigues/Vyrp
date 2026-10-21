// Provedores de transcrição e limpeza (V17b): tipos do Settings v4 e regras puras da tela.

import type { ApiProvider } from "./settingsApi";
import type { UiError } from "./uiError";

export type SttPolicy = "local_only" | "cloud_only" | "cloud_with_fallback";
export type SttCloud = "groq" | "open_ai";
export type LlmProviderId = "anthropic" | "open_ai" | "ollama";
export type CleanupLevel = "none" | "light" | "medium" | "high";
export type WritingStyle = "formal" | "casual" | "very_casual";

export const STT_POLICIES: SttPolicy[] = ["local_only", "cloud_only", "cloud_with_fallback"];
export const STT_CLOUDS: SttCloud[] = ["groq", "open_ai"];
export const LLM_PROVIDERS: LlmProviderId[] = ["anthropic", "open_ai", "ollama"];
export const CLEANUP_LEVELS: CleanupLevel[] = ["none", "light", "medium", "high"];
export const WRITING_STYLES: WritingStyle[] = ["formal", "casual", "very_casual"];

export const PROVIDER_NAMES: Record<ApiProvider | "ollama", string> = {
  groq: "Groq",
  open_ai: "OpenAI",
  anthropic: "Anthropic",
  ollama: "Ollama",
};

/** Modelo que o Vyrp usa quando `llm_model` é nulo (espelha src-tauri/src/llm/wire.rs). */
export const LLM_DEFAULT_MODEL: Record<LlmProviderId, string> = {
  anthropic: "claude-haiku-5-5",
  open_ai: "gpt-4o-mini",
  ollama: "llama3.2",
};

/** Endereço padrão do Ollama; ainda não é configurável no Settings. */
export const OLLAMA_URL = "http://localhost:11434";

export type KeyStatus = Partial<Record<ApiProvider, boolean | null>>;

export interface KeyWarning {
  provider: ApiProvider;
  scope: "stt" | "llm";
}

/** A política usa a nuvem (e portanto precisa de chave)? */
export function sttUsesCloud(policy: SttPolicy): boolean {
  return policy !== "local_only";
}

/** Chave do Credential Manager que o provedor de limpeza usa; Ollama local não usa. */
export function llmKeyFor(provider: LlmProviderId | null): ApiProvider | null {
  if (provider === "anthropic") return "anthropic";
  if (provider === "open_ai") return "open_ai";
  return null;
}

export interface ProviderChoices {
  stt_policy: SttPolicy;
  stt_cloud: SttCloud;
  llm_provider: LlmProviderId | null;
  cleanup: CleanupLevel;
}

/**
 * Escolhas que dependem de uma chave que sabidamente NÃO está configurada. Chave ainda
 * desconhecida (`null`/ausente) não gera aviso, para não piscar enquanto carrega.
 */
export function missingKeys(choices: ProviderChoices, status: KeyStatus): KeyWarning[] {
  const out: KeyWarning[] = [];
  if (sttUsesCloud(choices.stt_policy) && status[choices.stt_cloud] === false) {
    out.push({ provider: choices.stt_cloud, scope: "stt" });
  }
  const llmKey = llmKeyFor(choices.llm_provider);
  if (llmKey && choices.cleanup !== "none" && status[llmKey] === false) {
    out.push({ provider: llmKey, scope: "llm" });
  }
  return out;
}

/** Texto do campo de modelo → valor do Settings (vazio = padrão do provedor = null). */
export function normalizeModel(input: string): string | null {
  const trimmed = input.trim();
  return trimmed === "" ? null : trimmed;
}

/** Provedores de LLM em que o endereço pode ser trocado (servidor próprio / compatível). */
export function supportsCustomUrl(provider: LlmProviderId | null): boolean {
  return provider === "ollama" || provider === "open_ai";
}

/** Endereço padrão para mostrar como dica. */
export const LLM_DEFAULT_URL: Record<LlmProviderId, string> = {
  anthropic: "https://api.anthropic.com",
  open_ai: "https://api.openai.com",
  ollama: OLLAMA_URL,
};

/** Texto do campo de endereço → valor do Settings (vazio = padrão = null). */
export function normalizeUrl(input: string): string | null {
  const trimmed = input.trim();
  return trimmed === "" ? null : trimmed;
}

/** Modelo efetivo, para mostrar "usando X". */
export function effectiveModel(provider: LlmProviderId, model: string | null): string {
  return model ?? LLM_DEFAULT_MODEL[provider];
}

/** Como a escolha aparece no resumo da aba (ex.: "Groq" ou "desligada"). */
export function llmLabel(provider: LlmProviderId | null): string | null {
  return provider === null ? null : PROVIDER_NAMES[provider];
}

export interface ProviderTestResult {
  ok: boolean;
  /** Código + parâmetros; a tela traduz (`uiErrorText`). */
  message: UiError;
  ms: number;
}

export type ProviderTarget =
  | { kind: "stt_cloud"; provider: SttCloud }
  | { kind: "llm"; provider: LlmProviderId };
