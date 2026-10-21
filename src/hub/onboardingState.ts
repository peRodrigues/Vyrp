// Estado puro do onboarding (V20): etapas, regras de avanço, medidor de nível e eventos de download.

import type { DownloadProgress, ModelEntry } from "./modelsApi";

export const ONBOARDING_KEY = "vyrp.onboarding.done";

export const STEPS = ["welcome", "microphone", "model", "shortcut", "test", "done"] as const;
export type Step = (typeof STEPS)[number];

export function nextStep(step: Step): Step {
  const i = STEPS.indexOf(step);
  return STEPS[Math.min(i + 1, STEPS.length - 1)];
}

export function previousStep(step: Step): Step {
  const i = STEPS.indexOf(step);
  return STEPS[Math.max(i - 1, 0)];
}

/** Posição 1-based e total, para "Etapa 3 de 6". */
export function stepPosition(step: Step): { index: number; total: number } {
  return { index: STEPS.indexOf(step) + 1, total: STEPS.length };
}

export interface AdvanceContext {
  /** Há um modelo local instalado E ativo (ou nuvem configurada, decidido por quem chama). */
  hasUsableModel: boolean;
}

/** A etapa de modelo só avança com modelo pronto; as outras podem ser puladas. */
export function canAdvance(step: Step, ctx: AdvanceContext): boolean {
  if (step === "model") return ctx.hasUsableModel;
  return true;
}

// ---------------------------------------------------------------- medidor de nível

/** RMS (0..1) → largura da barra (0..100). Fala fica em ~0,05–0,2: escala por raiz, como o overlay. */
export function levelPercent(rms: number): number {
  if (!Number.isFinite(rms) || rms <= 0) return 0;
  return Math.round(Math.min(1, Math.sqrt(rms * 4)) * 100);
}

/** Pico mais alto visto até agora (o medidor mostra o instante; a dica usa o pico). */
export function trackPeak(peak: number, rms: number): number {
  return Number.isFinite(rms) ? Math.max(peak, rms) : peak;
}

/** Depois de ouvir um tempo, pico ainda mínimo = microfone mudo, errado ou sem permissão. */
export const SILENCE_PEAK = 0.01;
export const SILENCE_AFTER_MS = 4_000;

export function looksSilent(peak: number, listenedMs: number): boolean {
  return listenedMs >= SILENCE_AFTER_MS && peak < SILENCE_PEAK;
}

// ---------------------------------------------------------------- downloads

export type ProgressMap = Record<string, { downloaded: number; total: number }>;

export function applyProgress(map: ProgressMap, p: DownloadProgress): ProgressMap {
  return { ...map, [p.id]: { downloaded: p.downloaded, total: p.total } };
}

export function withoutKey<T>(map: Record<string, T>, id: string): Record<string, T> {
  const { [id]: removed, ...rest } = map;
  void removed;
  return rest;
}

/** 0..100 do download; total desconhecido = 0. */
export function downloadPercent(p: { downloaded: number; total: number } | undefined): number {
  if (!p || p.total <= 0) return 0;
  return Math.max(0, Math.min(100, Math.round((p.downloaded / p.total) * 100)));
}

/** Modelo que serve para ditar agora: o ativo, instalado. */
export function usableModel(models: ModelEntry[]): ModelEntry | undefined {
  return models.find((m) => m.active && m.installed);
}

/** O modelo sugerido pelo binário (GPU/CPU), se houver. */
export function recommendedModel(models: ModelEntry[]): ModelEntry | undefined {
  return models.find((m) => m.recommended);
}

// ---------------------------------------------------------------- "já fiz"
// A flag mora no Settings (`onboarding_done`). Versões anteriores guardavam no localStorage:
// lemos esse valor uma vez, passamos para o Settings e o apagamos.

/** Passar o valor antigo para o Settings? Só se ainda não está lá e o antigo diz "concluído". */
export function shouldMigrateLegacyFlag(settingsDone: boolean, legacyDone: boolean): boolean {
  return legacyDone && !settingsDone;
}

export function readLegacyOnboardingDone(): boolean {
  try {
    return window.localStorage.getItem(ONBOARDING_KEY) === "1";
  } catch {
    return false;
  }
}

export function clearLegacyOnboarding(): void {
  try {
    window.localStorage.removeItem(ONBOARDING_KEY);
  } catch {
    // sem localStorage: nada a limpar
  }
}
