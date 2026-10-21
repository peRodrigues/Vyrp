import { describe, expect, it } from "vitest";
import {
  errorName,
  formatLatency,
  hasData,
  latencyRows,
  percent,
  type Aggregate,
  type MetricsReport,
} from "./diagnosticsRules";

const summary = (p50: number | null) => ({ count: p50 === null ? 0 : 3, p50, p95: p50, max: p50 });

const aggregate: Aggregate = {
  total: 10,
  ok: 8,
  failed: 2,
  llm_used: 4,
  llm_fallbacks: 1,
  stt: summary(500),
  llm: summary(null),
  total_latency: summary(1_400),
  errors: [{ code: "stt_failed", count: 2 }],
};

describe("formatLatency", () => {
  it("ms abaixo de 1 s, segundos acima, traço sem dado", () => {
    expect(formatLatency(850)).toBe("850 ms");
    expect(formatLatency(999.4)).toBe("999 ms");
    expect(formatLatency(2_300)).toBe("2,3 s");
    expect(formatLatency(1_000)).toBe("1,0 s");
    expect(formatLatency(null)).toBe("—");
  });
});

describe("percent", () => {
  it("arredonda e protege contra divisão por zero", () => {
    expect(percent(1, 3)).toBe(33);
    expect(percent(8, 10)).toBe(80);
    expect(percent(5, 0)).toBe(0);
  });
});

describe("errorName", () => {
  it("traduz os códigos conhecidos e repete os novos", () => {
    expect(errorName("stt_failed")).toBe("Falha na transcrição");
    expect(errorName("insert_failed")).toBe("Não deu para colar");
    expect(errorName("blocked")).toBe("Bloqueado pela lista de privacidade");
    expect(errorName("novo_codigo")).toBe("novo_codigo");
  });
});

describe("hasData / latencyRows", () => {
  it("painel vazio quando não há ditados", () => {
    expect(hasData(null)).toBe(false);
    const empty: MetricsReport = {
      aggregate: { ...aggregate, total: 0 },
      recent_errors: [],
      sample_size: 0,
    };
    expect(hasData(empty)).toBe(false);
    expect(hasData({ ...empty, aggregate })).toBe(true);
  });

  it("linhas na ordem do pipeline", () => {
    expect(latencyRows(aggregate).map((r) => r.key)).toEqual(["stt", "llm", "total"]);
    expect(latencyRows(aggregate)[2].summary.p50).toBe(1_400);
  });
});
