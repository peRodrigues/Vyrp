import { describe, expect, it } from "vitest";
import { draftError, draftFromEntry, emptyDraft, importSummary, kindLabel } from "./dictionaryApi";
import type { DictionaryEntry } from "./dictionaryApi";

const entry = (over: Partial<DictionaryEntry>): DictionaryEntry => ({
  id: 1,
  phrase: "virp",
  replacement: "Vyrp",
  replacement_html: null,
  is_snippet: false,
  manual_entry: true,
  origin: null,
  observed_asr_error: null,
  frequency: 0,
  last_used_at: null,
  favorite: false,
  is_deleted: false,
  created_at: 0,
  ...over,
});

describe("draftError", () => {
  it("pede a frase", () => {
    expect(draftError(emptyDraft())).toBe("Digite a frase.");
    expect(draftError({ ...emptyDraft(), phrase: "   " })).toBe("Digite a frase.");
  });

  it("snippet exige expansão; vocabulário não", () => {
    const base = { ...emptyDraft(), phrase: "meu e-mail" };
    expect(draftError({ ...base, is_snippet: true })).toMatch(/expansão/);
    expect(draftError({ ...base, is_snippet: true, replacement: "ana@x.com" })).toBeNull();
    expect(draftError(base)).toBeNull();
  });
});

describe("kindLabel", () => {
  it("distingue snippet, substituição e vocabulário", () => {
    expect(kindLabel(entry({ is_snippet: true }))).toBe("Snippet");
    expect(kindLabel(entry({}))).toBe("Substituição");
    expect(kindLabel(entry({ replacement: null }))).toBe("Vocabulário");
  });
});

describe("draftFromEntry", () => {
  it("troca expansão nula por texto vazio para o formulário", () => {
    expect(draftFromEntry(entry({ replacement: null })).replacement).toBe("");
    expect(draftFromEntry(entry({ id: 7 })).id).toBe(7);
  });
});

describe("importSummary", () => {
  it("só cita ignoradas quando houver", () => {
    expect(importSummary({ added: 2, updated: 1, skipped: 0 })).toBe("2 adicionadas, 1 atualizadas");
    expect(importSummary({ added: 0, updated: 0, skipped: 3 })).toBe(
      "0 adicionadas, 0 atualizadas, 3 ignoradas",
    );
  });
});
