import { describe, expect, it } from "vitest";
import { BARS, barHeights, pushLevel, silence } from "./waveform";

describe("pushLevel", () => {
  it("mantém o tamanho e descarta o mais velho", () => {
    let h = silence(3);
    h = pushLevel(h, 0.1, 3);
    h = pushLevel(h, 0.2, 3);
    h = pushLevel(h, 0.3, 3);
    h = pushLevel(h, 0.4, 3);
    expect(h).toEqual([0.2, 0.3, 0.4]);
  });

  it("limita a 0..1 e troca NaN por 0", () => {
    expect(pushLevel([], 5, 2)).toEqual([1]);
    expect(pushLevel([], -1, 2)).toEqual([0]);
    expect(pushLevel([], Number.NaN, 2)).toEqual([0]);
  });

  it("tamanho padrão", () => {
    expect(silence()).toHaveLength(BARS);
    expect(pushLevel(silence(), 0.5)).toHaveLength(BARS);
  });
});

describe("barHeights", () => {
  it("silêncio na altura mínima, nível alto no máximo", () => {
    expect(barHeights([0, 1], 3, 24)).toEqual([3, 24]);
  });

  it("fala baixa já é visível e a escala é crescente", () => {
    const [low, mid, high] = barHeights([0.02, 0.1, 0.2], 3, 24);
    expect(low).toBeGreaterThan(3);
    expect(mid).toBeGreaterThan(low);
    expect(high).toBeGreaterThan(mid);
  });
});
