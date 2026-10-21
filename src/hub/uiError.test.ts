import { afterEach, describe, expect, it } from "vitest";
import { setLanguage } from "./i18n";
import { isUiError, rawError, toUiError, uiErrorText } from "./uiError";

afterEach(() => setLanguage("pt-BR"));

describe("uiErrorText", () => {
  it("traduz o código com os parâmetros, nos dois idiomas", () => {
    const err = { code: "provider.timeout", params: { provider: "Groq" } };
    expect(uiErrorText(err)).toBe("Tempo esgotado falando com o Groq.");
    setLanguage("en");
    expect(uiErrorText(err)).toBe("Timed out talking to Groq.");
  });

  it("texto cru passa como veio", () => {
    expect(uiErrorText(rawError("disco cheio"))).toBe("disco cheio");
    expect(uiErrorText("falhou")).toBe("falhou");
    expect(uiErrorText(new Error("boom"))).toBe("boom");
  });

  it("código sem tradução cai no texto de apoio, depois no próprio código", () => {
    expect(uiErrorText({ code: "novo.codigo", params: { detail: "x" } })).toBe("x");
    expect(uiErrorText({ code: "novo.codigo" })).toBe("novo.codigo");
  });

  it("valor desconhecido vira 'erro desconhecido'", () => {
    expect(uiErrorText(undefined)).toBe("erro desconhecido");
    setLanguage("en");
    expect(uiErrorText(42)).toBe("unknown error");
  });
});

describe("toUiError / isUiError", () => {
  it("reconhece o formato {code, params} e normaliza o resto", () => {
    expect(isUiError({ code: "a.b" })).toBe(true);
    expect(isUiError({ code: 1 })).toBe(false);
    expect(isUiError(null)).toBe(false);
    expect(toUiError({ code: "a.b" })).toEqual({ code: "a.b" });
    expect(toUiError("x")).toEqual({ code: "raw", params: { text: "x" } });
  });
});
