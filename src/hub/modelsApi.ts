// Contrato da Forja para o STT local (src-tauri/src/stt/mod.rs): commands e eventos dos modelos.

import { invoke } from "@tauri-apps/api/core";
import { hasKey, t } from "./i18n";
import type { UiError } from "./uiError";

export const EVENT_MODEL_PROGRESS = "model://progress";
export const EVENT_MODEL_DONE = "model://done";
export const EVENT_MODEL_ERROR = "model://error";
export const EVENT_STT_STATUS = "stt://status";

export interface ModelEntry {
  id: string;
  size_bytes: number;
  installed: boolean;
  downloading: boolean;
  /** É o `settings.stt_model`. */
  active: boolean;
  /** Sugestão para este binário (GPU ou CPU). */
  recommended: boolean;
}

export interface DownloadProgress {
  id: string;
  downloaded: number;
  total: number;
}

export interface DownloadResult {
  id: string;
  error: UiError | null;
}

export type LoadState = "none" | "loading" | "ready" | "error";

export interface SttStatus {
  model: string | null;
  state: LoadState;
  backend: string;
  load_ms: number | null;
  error: UiError | null;
}

export const modelsApi = {
  list: () => invoke<ModelEntry[]>("model_list"),
  download: (id: string) => invoke<void>("model_download", { id }),
  status: () => invoke<SttStatus>("model_status"),
  select: (id: string) => invoke<SttStatus>("model_select", { id }),
};

/** Nome do modelo no idioma atual (chave `modelname.<id>`); id desconhecido aparece como está. */
export function modelName(id: string): string {
  const key = `modelname.${id}`;
  return hasKey(key) ? t(key) : id;
}
