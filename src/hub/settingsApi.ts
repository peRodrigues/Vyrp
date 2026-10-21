// Espelho TS de src-tauri/src/settings/** e hub.rs, e as chamadas aos commands.

import { invoke } from "@tauri-apps/api/core";
import type { LearnMode } from "./learnApi";
import type { Privacy } from "./privacyRules";
import type {
  CleanupLevel,
  LlmProviderId,
  ProviderTarget,
  ProviderTestResult,
  SttCloud,
  SttPolicy,
  WritingStyle,
} from "./providerRules";

export type HistoryRetention = "keep" | "day" | "never";
export type ApiProvider = "groq" | "open_ai" | "anthropic";

export type VoiceAction = { kind: "insert"; text: string } | { kind: "undo_last" };

export interface VoiceCommand {
  phrases: string[];
  action: VoiceAction;
}

export interface GestureConfig {
  ptt: number[];
  lock: number[] | null;
  cancel: number;
  actions: unknown[];
  min_hold_ms: number;
  double_tap_ms: number;
  debounce_ms: number;
}

/**
 * Os campos conhecidos da tela. `set_settings` recebe o objeto inteiro, então
 * campos que outros cards acrescentarem passam intactos (index signature).
 */
export interface Settings {
  version: number;
  ui_language: string;
  dictation_languages: string[];
  microphone: string | null;
  mic_ranking: string[];
  warm_mic: boolean;
  preroll_ms: number;
  gestures: GestureConfig;
  history_retention: HistoryRetention;
  stt_policy: SttPolicy;
  stt_cloud: SttCloud;
  llm_provider: LlmProviderId | null;
  /** `null` = modelo padrão do provedor. */
  llm_model: string | null;
  /** Endereço da API do LLM; `null` = padrão do provedor. */
  llm_url: string | null;
  /** Onboarding concluído (ou pulado). */
  onboarding_done: boolean;
  /** Lista de privacidade e áudio no histórico. */
  privacy: Privacy;
  cleanup: CleanupLevel;
  style: WritingStyle;
  voice_commands: VoiceCommand[];
  sounds: boolean;
  /** Auto-aprendizado do dicionário (V24). */
  dictionary_learning: LearnMode;
  /** Manter as reticências que o STT põe nas pausas (padrão: não). */
  keep_ellipsis: boolean;
  [extra: string]: unknown;
}

export interface HistoryItem {
  id: string;
  created_at: number;
  raw_text: string;
  final_text: string;
  app: string | null;
  language: string | null;
  duration_ms: number;
  word_count: number;
  latency_total_ms: number | null;
  status: string;
  reverted_ai: boolean;
  has_audio: boolean;
}

export const SETTINGS_CHANGED = "settings://changed";

export const settingsApi = {
  get: () => invoke<Settings>("get_settings"),
  set: (settings: Settings) => invoke<Settings>("set_settings", { settings }),
  defaults: () => invoke<Settings>("settings_defaults"),
  microphones: () => invoke<string[]>("list_microphones"),
  hasKey: (provider: ApiProvider) => invoke<boolean>("has_api_key", { provider }),
  setKey: (provider: ApiProvider, value: string) => invoke<void>("set_api_key", { provider, value }),
  clearKey: (provider: ApiProvider) => invoke<void>("clear_api_key", { provider }),
};

/** Testes de áudio do onboarding (hub.rs). */
export interface DictationTest {
  text: string;
  language: string | null;
  audio_ms: number;
  decode_ms: number;
}

export const audioApi = {
  monitorStart: () => invoke<void>("audio_monitor_start"),
  monitorStop: () => invoke<void>("audio_monitor_stop"),
  testStart: () => invoke<void>("dictation_test_start"),
  testStop: () => invoke<DictationTest>("dictation_test_stop"),
};

export const providersApi = {
  test: (target: ProviderTarget) => invoke<ProviderTestResult>("provider_test", { target }),
};

export const historyApi = {
  list: (search: string, limit: number, offset: number) =>
    invoke<HistoryItem[]>("history_list", { search, limit, offset }),
  remove: (id: string) => invoke<boolean>("history_delete", { id }),
  clear: () => invoke<number>("history_clear"),
};
