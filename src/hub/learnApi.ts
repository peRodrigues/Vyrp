// Contrato do auto-aprendizado (src-tauri/src/learn/mod.rs): commands e o evento dictionary://learned.

import { invoke } from "@tauri-apps/api/core";
import type { DictionaryEntry } from "./dictionaryApi";

export const EVENT_LEARNED = "dictionary://learned";

export type LearnMode = "off" | "suggest" | "auto";
export const LEARN_MODES: LearnMode[] = ["off", "suggest", "auto"];

/** Sugestão pendente (modo `suggest`), só em memória no backend. */
export interface Suggestion {
  id: number;
  phrase: string;
  observed: string;
}

/** Payload do evento. `id`: sugestão (modo suggest) ou entrada criada (modo auto). */
export interface LearnedEvent {
  mode: LearnMode;
  id: number;
  phrase: string;
  observed: string;
}

/** Valor do Settings → modo válido (qualquer coisa desconhecida vale "suggest", o padrão). */
export const toLearnMode = (value: unknown): LearnMode =>
  LEARN_MODES.includes(value as LearnMode) ? (value as LearnMode) : "suggest";

export const learnApi = {
  suggestions: () => invoke<Suggestion[]>("learn_suggestions"),
  accept: (id: number) => invoke<DictionaryEntry>("learn_accept", { id }),
  dismiss: (id: number) => invoke<boolean>("learn_dismiss", { id }),
  undo: (entryId: number) => invoke<boolean>("learn_undo", { entryId }),
};

/** A entrada nasceu de uma correção do usuário? */
export const isLearned = (entry: Pick<DictionaryEntry, "origin">): boolean => entry.origin === "learned";
