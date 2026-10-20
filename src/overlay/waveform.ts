// Waveform do overlay: histórico curto de níveis (RMS 0..1) → altura das barras.

export const BARS = 20;

/** Acrescenta o nível mais novo à direita e descarta o mais velho. Valor inválido vira 0. */
export function pushLevel(history: number[], level: number, size: number = BARS): number[] {
  const safe = Number.isFinite(level) ? Math.min(Math.max(level, 0), 1) : 0;
  return [...history.slice(Math.max(0, history.length - size + 1)), safe];
}

export const silence = (size: number = BARS): number[] => Array.from({ length: size }, () => 0);

/**
 * Altura de cada barra em pixels. Fala costuma ter RMS baixo (~0,05–0,2), então a escala
 * é por raiz: sobe rápido no começo e satura em 1. Silêncio fica na altura mínima.
 */
export function barHeights(history: number[], minPx: number, maxPx: number): number[] {
  return history.map((level) => {
    const scaled = Math.min(1, Math.sqrt(Math.max(level, 0) * 4));
    return Math.round(minPx + (maxPx - minPx) * scaled);
  });
}
