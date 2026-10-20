import { describe, expect, it } from "vitest";
import { stateLabel } from "./dictation";

describe("stateLabel", () => {
  it("distingue ouvindo travado de PTT", () => {
    expect(stateLabel({ state: "listening", locked: false })).toBe("Ouvindo");
    expect(stateLabel({ state: "listening", locked: true })).toBe("Ouvindo (mãos livres)");
  });

  it("traduz códigos de erro", () => {
    expect(stateLabel({ state: "error", code: "no_audio" })).toBe("Nenhum áudio captado");
  });

  it("falha ao colar avisa que o texto foi copiado", () => {
    expect(stateLabel({ state: "error", code: "insert_failed" })).toBe("Copiado. Cole manualmente.");
  });

  it("tem texto para todos os estados", () => {
    for (const s of ["idle", "arming", "processing", "inserting", "done"] as const) {
      expect(stateLabel({ state: s }).length).toBeGreaterThan(0);
    }
  });
});
