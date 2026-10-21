import { describe, expect, it } from "vitest";
import { selectionDiff, setTransformChord, transformChord } from "./transformRules";
describe("transform shortcuts", () => {
  it("edits one slot and preserves other actions", () => {
    const actions: unknown[] = [[[17, 86], "paste_last"], [[17, 49], { transform_prompt: 1 }]];
    const next = setTransformChord(actions, 9, [17, 57]);
    expect(transformChord(next, 9)).toEqual([17, 57]);
    expect(setTransformChord(next, 9, null)).toEqual(actions);
    expect(transformChord(setTransformChord(next, null, [17, 18, 84]), null)).toEqual([17, 18, 84]);
    expect(actions).toHaveLength(2);
  });
});
describe("selection diff", () => {
  it("reconstructs both sides without splitting emoji or line breaks", () => {
    for (const [before, after] of [["ação 🦀\ntexto", "ação 🐈\nnovo"], ["", "added"], ["abc", ""], ["same", "same"]]) {
      const d = selectionDiff(before, after);
      expect(d.prefix + d.removed + d.suffix).toBe(before);
      expect(d.prefix + d.added + d.suffix).toBe(after);
    }
  });
});
