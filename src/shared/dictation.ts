// Espelho TS dos contratos de src-tauri/src/core/types.rs (eventos do ditado).

import { t } from "../hub/i18n";

export const EVENT_STATE = "dictation://state";
export const EVENT_LEVEL = "dictation://level";
/** Um ditado tomou (ou devolveu) o microfone: payload `{ active: boolean }`. */
export const EVENT_MONITOR = "audio://monitor";

export type ErrorCode =
  | "mic_unavailable"
  | "no_audio"
  | "stt_failed"
  | "llm_failed"
  | "insert_failed"
  | "timeout"
  | "blocked";

export type DictationState =
  | { state: "idle" }
  | { state: "arming" }
  | { state: "listening"; locked: boolean }
  | { state: "processing" }
  | { state: "inserting" }
  | { state: "done" }
  | { state: "error"; code: ErrorCode };

export interface AudioLevel {
  rms: number;
}

/** Texto curto do overlay para cada estado. */
export function stateLabel(s: DictationState): string {
  switch (s.state) {
    case "idle":
      return t("overlay.idle");
    case "arming":
      return t("overlay.arming");
    case "listening":
      return s.locked ? t("overlay.listeningLocked") : t("overlay.listening");
    case "processing":
      return t("overlay.processing");
    case "inserting":
      return t("overlay.inserting");
    case "done":
      return t("overlay.done");
    case "error":
      return t(`overlay.error.${s.code}`);
  }
}
