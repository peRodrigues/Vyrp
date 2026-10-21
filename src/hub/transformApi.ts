import { invoke } from "@tauri-apps/api/core";
export interface SavedPrompt { slot: number; title: string; instruction: string }
export interface Transformation {
  id: string; created_at: number; original_text: string; instruction: string;
  transformed_text: string; prompt_slot: number | null;
  status: "inserted" | "insert_failed" | "undone"; provider: string;
}
export type Phase = "idle" | "capturing" | "listening" | "processing" | "inserting";
export interface TransformStatus { phase: Phase; error: unknown | null; entry_id: string | null }
export const transformApi = {
  prompts: () => invoke<SavedPrompt[]>("transform_prompts"),
  save: (prompt: SavedPrompt) => invoke<void>("transform_save_prompt", { prompt }),
  history: (offset = 0) => invoke<Transformation[]>("transform_history", { offset }),
  delete: (id: string | null) => invoke<void>("transform_delete", { id }),
  undo: (id: string) => invoke<void>("transform_undo", { id }),
  status: () => invoke<TransformStatus>("transform_status"),
  cancel: () => invoke<void>("transform_cancel"),
};
