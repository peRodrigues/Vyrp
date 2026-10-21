import { hasKey, t } from "./i18n";

// Métricas e diagnóstico (V22): tipos do que o backend devolve e regras puras de apresentação.

export interface Summary {
  count: number;
  p50: number | null;
  p95: number | null;
  max: number | null;
}

export interface ErrorCount {
  code: string;
  count: number;
}

export interface Aggregate {
  total: number;
  ok: number;
  failed: number;
  llm_used: number;
  llm_fallbacks: number;
  stt: Summary;
  llm: Summary;
  total_latency: Summary;
  errors: ErrorCount[];
}

export interface RecentError {
  created_at: number;
  code: string;
}

export interface MetricsReport {
  aggregate: Aggregate;
  recent_errors: RecentError[];
  sample_size: number;
}

/** Quantos ditados o painel analisa, para o seletor. */
export const SAMPLE_SIZES = [20, 50, 100, 200] as const;
export const DEFAULT_SAMPLE = 50;

/** 850 → "850 ms"; 2300 → "2,3 s"; sem valor → "—". */
export function formatLatency(ms: number | null): string {
  if (ms === null) return "—";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(1).replace(".", ",")} s`;
}

/** Porcentagem inteira de `part` em `whole` (0 quando não há `whole`). */
export function percent(part: number, whole: number): number {
  return whole <= 0 ? 0 : Math.round((part / whole) * 100);
}

/** Nome (no idioma da interface) de um código de erro do histórico; código novo aparece como veio. */
export function errorName(code: string): string {
  const key = `err.${code}`;
  return hasKey(key) ? t(key) : code;
}

/** Há dados para mostrar? (Painel vazio mostra uma explicação, não uma tabela de traços.) */
export function hasData(report: MetricsReport | null): boolean {
  return report !== null && report.aggregate.total > 0;
}

export interface LatencyRow {
  key: "stt" | "llm" | "total";
  summary: Summary;
}

/** Linhas da tabela, na ordem do pipeline. */
export function latencyRows(a: Aggregate): LatencyRow[] {
  return [
    { key: "stt", summary: a.stt },
    { key: "llm", summary: a.llm },
    { key: "total", summary: a.total_latency },
  ];
}
