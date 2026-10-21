import { afterEach, describe, expect, it } from "vitest";
import { en } from "./lang/en";
import { ptBR } from "./lang/pt-BR";
import {
  LANGUAGES,
  decimal,
  getLanguage,
  hasKey,
  resolveLanguage,
  setLanguage,
  t,
  type StringKey,
} from "./i18n";

const PT_KEYS = Object.keys(ptBR).sort();
const EN_KEYS = Object.keys(en).sort();

const placeholders = (text: string): string[] =>
  [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

afterEach(() => setLanguage("pt-BR"));

describe("paridade dos dicionários", () => {
  it("toda chave existe nos dois idiomas", () => {
    const onlyPt = PT_KEYS.filter((k) => !(k in en));
    const onlyEn = EN_KEYS.filter((k) => !(k in ptBR));
    expect(onlyPt, `faltam em en.ts: ${onlyPt.join(", ")}`).toEqual([]);
    expect(onlyEn, `sobram em en.ts (não existem em pt-BR.ts): ${onlyEn.join(", ")}`).toEqual([]);
  });

  it("os {placeholders} são os mesmos nos dois idiomas", () => {
    for (const key of PT_KEYS) {
      expect(placeholders(en[key as StringKey]), key).toEqual(placeholders(ptBR[key as StringKey]));
    }
  });

  it("nenhum texto vazio nem igual à própria chave", () => {
    for (const [name, dict] of [["pt-BR", ptBR], ["en", en]] as const) {
      for (const [key, value] of Object.entries(dict)) {
        expect(value.trim(), `${name}:${key} vazio`).not.toBe("");
        expect(value, `${name}:${key} igual à chave`).not.toBe(key);
      }
    }
  });

  it("o inglês não é cópia do português nas frases longas", () => {
    const copied = PT_KEYS.filter((k) => {
      const pt = ptBR[k as StringKey];
      return pt.length > 40 && pt === en[k as StringKey];
    });
    expect(copied, `sem tradução: ${copied.join(", ")}`).toEqual([]);
  });
});

describe("resolveLanguage", () => {
  it("o idioma escolhido vale, 'auto' segue o do sistema", () => {
    expect(resolveLanguage("en", "pt-BR")).toBe("en");
    expect(resolveLanguage("pt-BR", "en-US")).toBe("pt-BR");
    expect(resolveLanguage("auto", "pt-PT")).toBe("pt-BR");
    expect(resolveLanguage("auto", "PT")).toBe("pt-BR");
    expect(resolveLanguage("auto", "en-GB")).toBe("en");
    expect(resolveLanguage("auto", "de-DE")).toBe("en");
  });

  it("valor desconhecido ou ausente cai no idioma do sistema", () => {
    expect(resolveLanguage("klingon", "pt-BR")).toBe("pt-BR");
    expect(resolveLanguage(undefined, "fr-FR")).toBe("en");
    expect(resolveLanguage(null, undefined)).toBe("en");
  });

  it("os idiomas suportados são pt-BR e en", () => {
    expect(LANGUAGES).toEqual(["pt-BR", "en"]);
  });
});

describe("t / setLanguage", () => {
  it("troca o texto conforme o idioma e interpola variáveis", () => {
    expect(getLanguage()).toBe("pt-BR");
    expect(t("tab.general")).toBe("Geral");
    setLanguage("en");
    expect(t("tab.general")).toBe("General");
    expect(t("common.saveError", { error: "x" })).toBe("Could not save: x");
    setLanguage("pt-BR");
    expect(t("common.saveError", { error: "x" })).toBe("Não foi possível salvar: x");
    expect(t("common.saveError")).toBe("Não foi possível salvar: {error}");
  });

  it("hasKey reconhece só chaves reais", () => {
    expect(hasKey("err.stt_failed")).toBe(true);
    expect(hasKey("err.inventado")).toBe(false);
  });

  it("separador decimal segue o idioma", () => {
    expect(decimal("1.5")).toBe("1,5");
    setLanguage("en");
    expect(decimal("1.5")).toBe("1.5");
  });
});

// ------------------------------------------------------------------ varredura de strings soltas

const sources = import.meta.glob(["../hub/*.tsx", "../overlay/*.tsx"], {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

function withoutComments(code: string): string {
  return code.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
}

const LETTERS = /\p{L}{2,}/u;
/** Nome da marca e nomes de idioma (endônimos) não se traduzem. */
const UNTRANSLATED = new Set(["Vyrp", "Português (Brasil)", "English"]);

describe("sem texto solto fora do t()", () => {
  it("encontra os componentes para varrer", () => {
    expect(Object.keys(sources).length).toBeGreaterThan(8);
  });

  it("nenhum texto literal entre tags JSX", () => {
    const offenders: string[] = [];
    for (const [file, raw] of Object.entries(sources)) {
      const code = withoutComments(raw);
      for (const m of code.matchAll(/(?<!=)>([^<>{}=;]+)</g)) {
        const text = m[1].trim();
        if (text !== "" && LETTERS.test(text) && !UNTRANSLATED.has(text)) offenders.push(`${file}: "${text}"`);
      }
    }
    expect(offenders, offenders.join("\n")).toEqual([]);
  });

  it("nenhum placeholder/title/aria-label literal", () => {
    const offenders: string[] = [];
    for (const [file, raw] of Object.entries(sources)) {
      const code = withoutComments(raw);
      for (const m of code.matchAll(/\b(placeholder|title|aria-label|alt)="([^"]*)"/g)) {
        if (LETTERS.test(m[2])) offenders.push(`${file}: ${m[1]}="${m[2]}"`);
      }
    }
    expect(offenders, offenders.join("\n")).toEqual([]);
  });
});

// ------------------------------------------------------------------ códigos de erro do Rust

const rustSources = import.meta.glob("../../src-tauri/src/**/*.rs", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/** Os prefixos dos códigos estáveis de `UiError` (src-tauri/src/core/ui_error.rs). */
const CODE = /"((?:provider|stt|llm|test|settings|shortcut|key|dictionary|log|model|learn)\.(?!json")[a-z_]+)"/g;

function rustCodes(): Set<string> {
  const codes = new Set<string>();
  for (const [file, raw] of Object.entries(rustSources)) {
    if (file.endsWith("tray_i18n.rs")) continue;
    for (const m of raw.matchAll(CODE)) codes.add(m[1]);
  }
  return codes;
}

describe("erros do Rust traduzidos", () => {
  it("encontra os fontes e códigos para conferir", () => {
    expect(Object.keys(rustSources).length).toBeGreaterThan(20);
    expect(rustCodes().size).toBeGreaterThan(25);
  });

  it("todo código usado no Rust tem texto nos dois idiomas", () => {
    const missing = [...rustCodes()].filter((c) => !(`uierr.${c}` in ptBR) || !(`uierr.${c}` in en));
    expect(missing, `sem uierr.<código>: ${missing.join(", ")}`).toEqual([]);
  });

  it("nenhuma chave uierr.* sobra sem uso no Rust", () => {
    const used = rustCodes();
    const stale = PT_KEYS.filter((k) => k.startsWith("uierr.") && !used.has(k.slice("uierr.".length)));
    expect(stale, `sobram: ${stale.join(", ")}`).toEqual([]);
  });

  it("todo modelo do catálogo tem nome nos dois idiomas", () => {
    const catalog = Object.entries(rustSources).find(([file]) => file.endsWith("stt/catalog.rs"))?.[1] ?? "";
    const ids = [...catalog.matchAll(/^\s+id: "([^"]+)",/gm)].map((m) => m[1]);
    expect(ids.length).toBeGreaterThanOrEqual(6);
    const missing = ids.filter((id) => !(`modelname.${id}` in ptBR) || !(`modelname.${id}` in en));
    expect(missing, `sem modelname.<id>: ${missing.join(", ")}`).toEqual([]);
  });
});
