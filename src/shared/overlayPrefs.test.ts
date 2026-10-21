import { describe, expect, it } from "vitest";
import {
  HIDE_ONE_HOUR_MS,
  hideFor,
  hideForever,
  isHidden,
  msUntilVisible,
  overlayVisible,
  parsePref,
  serializePref,
} from "./overlayPrefs";

describe("parsePref / serializePref", () => {
  it("ida e volta", () => {
    for (const pref of [hideForever(), hideFor(1000, 5)]) {
      expect(parsePref(serializePref(pref))).toEqual(pref);
    }
    expect(serializePref(null)).toBeNull();
  });

  it("lixo vira null", () => {
    for (const raw of [null, "", "x", "42", "null", '{"mode":"until"}', '{"mode":"until","until":"a"}', '{"mode":"x"}']) {
      expect(parsePref(raw)).toBeNull();
    }
  });
});

describe("isHidden / msUntilVisible", () => {
  it("sempre oculta até alguém mudar", () => {
    expect(isHidden(hideForever(), 9e15)).toBe(true);
    expect(msUntilVisible(hideForever(), 0)).toBeNull();
  });

  it("por 1 h: oculta antes do prazo, visível depois", () => {
    const pref = hideFor(HIDE_ONE_HOUR_MS, 1000);
    expect(isHidden(pref, 1000 + HIDE_ONE_HOUR_MS - 1)).toBe(true);
    expect(isHidden(pref, 1000 + HIDE_ONE_HOUR_MS)).toBe(false);
    expect(msUntilVisible(pref, 2000)).toBe(HIDE_ONE_HOUR_MS - 1000);
    expect(msUntilVisible(pref, 1000 + HIDE_ONE_HOUR_MS + 5)).toBe(0);
  });

  it("sem preferência: visível", () => {
    expect(isHidden(null, 0)).toBe(false);
    expect(msUntilVisible(null, 0)).toBeNull();
  });
});

describe("overlayVisible", () => {
  it("oculta só some em repouso; estados ativos sempre aparecem", () => {
    for (const pref of [hideForever(), hideFor(HIDE_ONE_HOUR_MS, 0)]) {
      expect(overlayVisible(pref, 10, "idle")).toBe(false);
      for (const state of ["arming", "listening", "processing", "inserting", "done", "error"]) {
        expect(overlayVisible(pref, 10, state)).toBe(true);
      }
    }
    expect(overlayVisible(null, 10, "idle")).toBe(true);
    expect(overlayVisible(hideFor(5, 0), 10, "idle")).toBe(true);
  });
});
