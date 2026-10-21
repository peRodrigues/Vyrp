import { describe, expect, it } from "vitest";
import {
  LLM_DEFAULT_MODEL,
  effectiveModel,
  llmKeyFor,
  LLM_DEFAULT_URL,
  llmLabel,
  missingKeys,
  normalizeModel,
  normalizeUrl,
  supportsCustomUrl,
  sttUsesCloud,
  type ProviderChoices,
} from "./providerRules";

const base: ProviderChoices = {
  stt_policy: "local_only",
  stt_cloud: "groq",
  llm_provider: null,
  cleanup: "light",
};

describe("sttUsesCloud", () => {
  it("só 'local_only' dispensa a nuvem", () => {
    expect(sttUsesCloud("local_only")).toBe(false);
    expect(sttUsesCloud("cloud_only")).toBe(true);
    expect(sttUsesCloud("cloud_with_fallback")).toBe(true);
  });
});

describe("llmKeyFor", () => {
  it("Ollama local não tem chave", () => {
    expect(llmKeyFor("anthropic")).toBe("anthropic");
    expect(llmKeyFor("open_ai")).toBe("open_ai");
    expect(llmKeyFor("ollama")).toBeNull();
    expect(llmKeyFor(null)).toBeNull();
  });
});

describe("missingKeys", () => {
  it("sem nuvem e sem LLM não há o que avisar", () => {
    expect(missingKeys(base, { groq: false, anthropic: false })).toEqual([]);
  });

  it("avisa da chave de transcrição quando a política usa a nuvem", () => {
    const choices = { ...base, stt_policy: "cloud_with_fallback" as const, stt_cloud: "open_ai" as const };
    expect(missingKeys(choices, { open_ai: false })).toEqual([{ provider: "open_ai", scope: "stt" }]);
    expect(missingKeys(choices, { open_ai: true })).toEqual([]);
  });

  it("avisa da chave do LLM, exceto Ollama ou limpeza desligada", () => {
    const llm = { ...base, llm_provider: "anthropic" as const };
    expect(missingKeys(llm, { anthropic: false })).toEqual([{ provider: "anthropic", scope: "llm" }]);
    expect(missingKeys({ ...llm, cleanup: "none" }, { anthropic: false })).toEqual([]);
    expect(missingKeys({ ...base, llm_provider: "ollama" }, {})).toEqual([]);
  });

  it("pode avisar das duas ao mesmo tempo, mesmo com a mesma chave", () => {
    const both: ProviderChoices = {
      stt_policy: "cloud_only",
      stt_cloud: "open_ai",
      llm_provider: "open_ai",
      cleanup: "medium",
    };
    expect(missingKeys(both, { open_ai: false })).toEqual([
      { provider: "open_ai", scope: "stt" },
      { provider: "open_ai", scope: "llm" },
    ]);
  });

  it("chave ainda carregando não gera aviso", () => {
    const choices = { ...base, stt_policy: "cloud_only" as const };
    expect(missingKeys(choices, { groq: null })).toEqual([]);
    expect(missingKeys(choices, {})).toEqual([]);
  });
});

describe("modelo", () => {
  it("campo vazio volta ao padrão (null)", () => {
    expect(normalizeModel("")).toBeNull();
    expect(normalizeModel("   ")).toBeNull();
    expect(normalizeModel(" qwen3:8b ")).toBe("qwen3:8b");
  });

  it("mostra o padrão do provedor quando não há modelo", () => {
    expect(effectiveModel("anthropic", null)).toBe("claude-haiku-5-5");
    expect(effectiveModel("ollama", "qwen3:8b")).toBe("qwen3:8b");
    expect(LLM_DEFAULT_MODEL.open_ai).toBe("gpt-4o-mini");
  });
});

describe("llmLabel", () => {
  it("nome do provedor ou null", () => {
    expect(llmLabel("open_ai")).toBe("OpenAI");
    expect(llmLabel(null)).toBeNull();
  });
});

describe("endereço do LLM", () => {
  it("só Ollama e OpenAI aceitam endereço próprio", () => {
    expect(supportsCustomUrl("ollama")).toBe(true);
    expect(supportsCustomUrl("open_ai")).toBe(true);
    expect(supportsCustomUrl("anthropic")).toBe(false);
    expect(supportsCustomUrl(null)).toBe(false);
  });

  it("campo vazio volta ao padrão (null)", () => {
    expect(normalizeUrl("  ")).toBeNull();
    expect(normalizeUrl(" http://10.0.0.5:11434 ")).toBe("http://10.0.0.5:11434");
    expect(LLM_DEFAULT_URL.ollama).toBe("http://localhost:11434");
  });
});
