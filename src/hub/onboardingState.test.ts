import { describe, expect, it } from "vitest";
import {
  SILENCE_AFTER_MS,
  STEPS,
  applyProgress,
  canAdvance,
  downloadPercent,
  levelPercent,
  looksSilent,
  nextStep,
  previousStep,
  recommendedModel,
  shouldMigrateLegacyFlag,
  stepPosition,
  trackPeak,
  usableModel,
  withoutKey,
} from "./onboardingState";
import type { ModelEntry } from "./modelsApi";

const model = (over: Partial<ModelEntry>): ModelEntry => ({
  id: "small",
  size_bytes: 466_000_000,
  installed: false,
  downloading: false,
  active: false,
  recommended: false,
  ...over,
});

describe("etapas", () => {
  it("anda para frente e para trás sem sair dos limites", () => {
    expect(nextStep("welcome")).toBe("microphone");
    expect(nextStep("done")).toBe("done");
    expect(previousStep("microphone")).toBe("welcome");
    expect(previousStep("welcome")).toBe("welcome");
  });

  it("começa nas boas-vindas e termina em pronto", () => {
    expect(STEPS[0]).toBe("welcome");
    expect(STEPS[STEPS.length - 1]).toBe("done");
    expect(stepPosition("model")).toEqual({ index: 3, total: STEPS.length });
  });

  it("só o modelo exige algo para avançar", () => {
    expect(canAdvance("model", { hasUsableModel: false })).toBe(false);
    expect(canAdvance("model", { hasUsableModel: true })).toBe(true);
    for (const s of STEPS.filter((s) => s !== "model")) {
      expect(canAdvance(s, { hasUsableModel: false })).toBe(true);
    }
  });
});

describe("medidor de nível", () => {
  it("escala por raiz e limita em 0..100", () => {
    expect(levelPercent(0)).toBe(0);
    expect(levelPercent(-1)).toBe(0);
    expect(levelPercent(Number.NaN)).toBe(0);
    expect(levelPercent(1)).toBe(100);
    expect(levelPercent(0.05)).toBeGreaterThan(40);
    expect(levelPercent(0.2)).toBeGreaterThan(levelPercent(0.05));
  });

  it("pico só sobe e ignora lixo", () => {
    expect(trackPeak(0.1, 0.05)).toBe(0.1);
    expect(trackPeak(0.1, 0.3)).toBe(0.3);
    expect(trackPeak(0.1, Number.NaN)).toBe(0.1);
  });

  it("avisa de silêncio só depois de ouvir um tempo", () => {
    expect(looksSilent(0, SILENCE_AFTER_MS - 1)).toBe(false);
    expect(looksSilent(0.001, SILENCE_AFTER_MS)).toBe(true);
    expect(looksSilent(0.2, SILENCE_AFTER_MS * 3)).toBe(false);
  });
});

describe("downloads", () => {
  it("aplica e remove progresso por modelo", () => {
    let map = applyProgress({}, { id: "small", downloaded: 50, total: 200 });
    map = applyProgress(map, { id: "base", downloaded: 1, total: 10 });
    map = applyProgress(map, { id: "small", downloaded: 100, total: 200 });
    expect(map.small).toEqual({ downloaded: 100, total: 200 });
    expect(Object.keys(withoutKey(map, "small"))).toEqual(["base"]);
    expect(withoutKey(map, "nada")).toEqual(map);
  });

  it("calcula a porcentagem com guardas", () => {
    expect(downloadPercent(undefined)).toBe(0);
    expect(downloadPercent({ downloaded: 5, total: 0 })).toBe(0);
    expect(downloadPercent({ downloaded: 50, total: 200 })).toBe(25);
    expect(downloadPercent({ downloaded: 999, total: 200 })).toBe(100);
  });

  it("acha o modelo utilizável e o recomendado", () => {
    const list = [
      model({ id: "tiny", installed: true }),
      model({ id: "small", recommended: true }),
      model({ id: "base", installed: true, active: true }),
    ];
    expect(usableModel(list)?.id).toBe("base");
    expect(usableModel([model({ active: true })])).toBeUndefined();
    expect(recommendedModel(list)?.id).toBe("small");
    expect(recommendedModel([])).toBeUndefined();
  });
});

describe("migração da flag antiga", () => {
  it("só migra quando o antigo diz concluído e o Settings ainda não", () => {
    expect(shouldMigrateLegacyFlag(false, true)).toBe(true);
    expect(shouldMigrateLegacyFlag(true, true)).toBe(false);
    expect(shouldMigrateLegacyFlag(false, false)).toBe(false);
    expect(shouldMigrateLegacyFlag(true, false)).toBe(false);
  });
});
