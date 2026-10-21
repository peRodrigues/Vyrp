// Preferência "ocultar a barra de ditado" (V16), guardada em localStorage. Hub e overlay
// são duas janelas da mesma origem: compartilham o valor e recebem o evento `storage`.

export const OVERLAY_PREF_KEY = "vyrp.overlay.hidden";
export const HIDE_ONE_HOUR_MS = 60 * 60 * 1000;

export type HiddenPref = { mode: "forever" } | { mode: "until"; until: number } | null;

export function parsePref(raw: string | null): HiddenPref {
  if (!raw) return null;
  try {
    const value: unknown = JSON.parse(raw);
    if (typeof value !== "object" || value === null) return null;
    const pref = value as { mode?: unknown; until?: unknown };
    if (pref.mode === "forever") return { mode: "forever" };
    if (pref.mode === "until" && typeof pref.until === "number" && Number.isFinite(pref.until)) {
      return { mode: "until", until: pref.until };
    }
  } catch {
    // valor corrompido: trata como "não oculta"
  }
  return null;
}

export function serializePref(pref: HiddenPref): string | null {
  return pref === null ? null : JSON.stringify(pref);
}

export function isHidden(pref: HiddenPref, now: number): boolean {
  if (pref === null) return false;
  return pref.mode === "forever" || pref.until > now;
}

/**
 * A barra aparece agora? "Ocultar" vale só em repouso: em qualquer estado ativo (ouvindo,
 * processando, colando, pronto, erro, transform) ela aparece e volta a sumir no idle.
 */
export function overlayVisible(pref: HiddenPref, now: number, state: string): boolean {
  return state !== "idle" || !isHidden(pref, now);
}

export const hideForever = (): HiddenPref => ({ mode: "forever" });
export const hideFor = (ms: number, now: number): HiddenPref => ({ mode: "until", until: now + ms });

/** Quanto falta para a barra voltar sozinha; `null` = não volta sozinha (ou já está visível). */
export function msUntilVisible(pref: HiddenPref, now: number): number | null {
  if (pref === null || pref.mode === "forever") return null;
  return Math.max(0, pref.until - now);
}

export function readPref(): HiddenPref {
  try {
    return parsePref(window.localStorage.getItem(OVERLAY_PREF_KEY));
  } catch {
    return null;
  }
}

export function writePref(pref: HiddenPref): void {
  try {
    const raw = serializePref(pref);
    if (raw === null) window.localStorage.removeItem(OVERLAY_PREF_KEY);
    else window.localStorage.setItem(OVERLAY_PREF_KEY, raw);
  } catch {
    // sem localStorage: a barra só não lembra da escolha
  }
}
