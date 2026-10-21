import { describe, expect, it } from "vitest";
import { LEARN_MODES, isLearned, toLearnMode } from "./learnApi";

describe("toLearnMode", () => {
  it("aceita os três modos e cai em 'suggest' (o padrão do backend) no resto", () => {
    for (const mode of LEARN_MODES) expect(toLearnMode(mode)).toBe(mode);
    expect(toLearnMode(undefined)).toBe("suggest");
    expect(toLearnMode("tudo")).toBe("suggest");
  });
});

describe("isLearned", () => {
  it("só as entradas de origem 'learned' são marcadas", () => {
    expect(isLearned({ origin: "learned" })).toBe(true);
    expect(isLearned({ origin: "manual" })).toBe(false);
    expect(isLearned({ origin: null })).toBe(false);
  });
});
