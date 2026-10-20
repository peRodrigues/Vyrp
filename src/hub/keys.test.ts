import { describe, expect, it } from "vitest";
import { chordFromCodes, chordLabel, codeToVk, mouseButtonCode, vkName } from "./keys";

describe("codeToVk", () => {
  it("mapeia modificadores, letras, dígitos e F-keys", () => {
    expect(codeToVk("ControlLeft")).toBe(0xa2);
    expect(codeToVk("MetaRight")).toBe(0x5c);
    expect(codeToVk("KeyA")).toBe(0x41);
    expect(codeToVk("Digit7")).toBe(0x37);
    expect(codeToVk("F1")).toBe(0x70);
    expect(codeToVk("F24")).toBe(0x87);
    expect(codeToVk("Space")).toBe(0x20);
  });

  it("recusa o que não conhece", () => {
    expect(codeToVk("F25")).toBeNull();
    expect(codeToVk("MediaPlayPause")).toBeNull();
    expect(codeToVk("Escape")).toBeNull();
  });
});

describe("mouseButtonCode", () => {
  it("só os botões 4 e 5", () => {
    expect(mouseButtonCode(3)).toBe("XButton1");
    expect(mouseButtonCode(4)).toBe("XButton2");
    expect(codeToVk("XButton2")).toBe(0x06);
    expect(mouseButtonCode(0)).toBeNull();
  });
});

describe("vkName / chordLabel", () => {
  it("nomeia modificadores genéricos e específicos", () => {
    expect(vkName(0x11)).toBe("Ctrl");
    expect(vkName(0x0100)).toBe("Win");
    expect(vkName(0xa3)).toBe("Ctrl dir.");
    expect(vkName(0x41)).toBe("A");
    expect(vkName(0x71)).toBe("F2");
    expect(vkName(0xfe)).toBe("0xFE");
  });

  it("junta o atalho com +", () => {
    expect(chordLabel([0x11, 0x0100, 0x20])).toBe("Ctrl + Win + Espaço");
  });
});

describe("chordFromCodes", () => {
  it("aceita modificador + tecla", () => {
    expect(chordFromCodes(["ControlLeft", "ShiftLeft", "KeyD"])).toEqual({
      chord: [0xa2, 0xa0, 0x44],
    });
  });

  it("ignora tecla repetida", () => {
    expect(chordFromCodes(["AltLeft", "AltLeft", "KeyX"])).toEqual({ chord: [0xa4, 0x58] });
  });

  it("exige um modificador", () => {
    const r = chordFromCodes(["KeyA", "KeyB"]);
    expect("error" in r && r.error).toMatch(/precisa de Ctrl/);
  });

  it("limita a 3 teclas e recusa tecla desconhecida ou vazio", () => {
    const four = chordFromCodes(["ControlLeft", "ShiftLeft", "AltLeft", "KeyA"]);
    expect("error" in four && four.error).toMatch(/no máximo 3/);
    const odd = chordFromCodes(["ControlLeft", "AudioVolumeUp"]);
    expect("error" in odd && odd.error).toMatch(/AudioVolumeUp/);
    expect("error" in chordFromCodes([])).toBe(true);
  });
});
