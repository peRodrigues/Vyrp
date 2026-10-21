import { describe, expect, it } from "vitest";
import { addEntry, cleanEntry, removeEntry } from "./privacyRules";

describe("cleanEntry", () => {
  it("apps: nome do exe em minúsculas, sem caminho nem .exe", () => {
    expect(cleanEntry("app", "KeePass.exe")).toBe("keepass");
    expect(cleanEntry("app", String.raw`C:\Program Files\KeePass\KeePass.EXE`)).toBe("keepass");
    expect(cleanEntry("app", "  1Password ")).toBe("1password");
    expect(cleanEntry("app", "   ")).toBe("");
  });

  it("sites: só o host, sem esquema, www, porta ou caminho", () => {
    expect(cleanEntry("domain", "https://www.Banco.com:8443/login?x=1")).toBe("banco.com");
    expect(cleanEntry("domain", "user@vault.bitwarden.com/x")).toBe("vault.bitwarden.com");
    expect(cleanEntry("domain", "*.exemplo.com.br")).toBe("exemplo.com.br");
    expect(cleanEntry("domain", "exemplo.com/caminho")).toBe("exemplo.com");
    expect(cleanEntry("domain", "")).toBe("");
  });
});

describe("addEntry / removeEntry", () => {
  it("acrescenta sem repetir e recusa vazio", () => {
    const first = addEntry([], "app", "Banco.exe");
    expect(first).toEqual({ list: ["banco"] });
    expect(addEntry(["banco"], "app", "BANCO")).toEqual({ error: "duplicate" });
    expect(addEntry(["banco"], "app", "  ")).toEqual({ error: "empty" });
    expect(addEntry(["a.com"], "domain", "https://b.com/x")).toEqual({ list: ["a.com", "b.com"] });
  });

  it("remove só a entrada pedida", () => {
    expect(removeEntry(["a", "b", "c"], "b")).toEqual(["a", "c"]);
    expect(removeEntry(["a"], "z")).toEqual(["a"]);
  });
});
