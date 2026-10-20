// Espelho TS dos contratos de src-tauri/src/core/types.rs (eventos do ditado).

export const EVENT_STATE = "dictation://state";
export const EVENT_LEVEL = "dictation://level";

export type ErrorCode =
  | "mic_unavailable"
  | "no_audio"
  | "stt_failed"
  | "llm_failed"
  | "insert_failed"
  | "timeout";

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

const ERROR_LABELS: Record<ErrorCode, string> = {
  mic_unavailable: "Microfone indisponível",
  no_audio: "Nenhum áudio captado",
  stt_failed: "Falha na transcrição",
  llm_failed: "Falha na limpeza",
  insert_failed: "Não deu para colar",
  timeout: "Demorou demais",
};

/** Texto curto do overlay para cada estado. */
export function stateLabel(s: DictationState): string {
  switch (s.state) {
    case "idle":
      return "Pronto";
    case "arming":
      return "Preparando…";
    case "listening":
      return s.locked ? "Ouvindo (mãos livres)" : "Ouvindo";
    case "processing":
      return "Processando…";
    case "inserting":
      return "Colando…";
    case "done":
      return "Pronto";
    case "error":
      return ERROR_LABELS[s.code];
  }
}
