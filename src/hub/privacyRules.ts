// Lista de privacidade (V21): regras puras da tela. A normalização de verdade é do backend
// (src-tauri/src/privacy.rs); aqui só evitamos entradas óbvias e mostramos os padrões.

export interface Privacy {
  block_defaults: boolean;
  apps: string[];
  domains: string[];
  save_audio: boolean;
}

/** Espelho de `DEFAULT_APPS` / `DEFAULT_DOMAINS` do backend, só para exibir. */
export const DEFAULT_APPS = [
  "1Password",
  "KeePass",
  "KeePassXC",
  "Bitwarden",
  "LastPass",
  "Dashlane",
  "Enpass",
  "NordPass",
  "RoboForm",
  "Keeper",
  "Proton Pass",
];
export const DEFAULT_DOMAINS = [
  "1password.com",
  "bitwarden.com",
  "lastpass.com",
  "dashlane.com",
  "keepersecurity.com",
  "nordpass.com",
  "enpass.io",
  "roboform.com",
];

export type EntryKind = "app" | "domain";

/** `C:\Programas\KeePass.exe` → `keepass`; `https://www.Banco.com/x` → `banco.com`. */
export function cleanEntry(kind: EntryKind, input: string): string {
  const raw = input.trim().toLowerCase();
  if (kind === "app") {
    const base = raw.split(/[\\/]/).pop() ?? "";
    return base.replace(/\.exe$/, "").trim();
  }
  const noScheme = raw.replace(/^[a-z][a-z0-9+.-]*:\/\//, "");
  const host = noScheme.split(/[/?#]/)[0].split("@").pop() ?? "";
  return host.split(":")[0].replace(/^\*\./, "").replace(/^www\./, "").replace(/^\.+|\.+$/g, "");
}

export type AddResult = { list: string[] } | { error: "empty" | "duplicate" };

/** Acrescenta uma entrada limpa à lista, sem repetir. */
export function addEntry(list: string[], kind: EntryKind, input: string): AddResult {
  const entry = cleanEntry(kind, input);
  if (entry === "") return { error: "empty" };
  if (list.includes(entry)) return { error: "duplicate" };
  return { list: [...list, entry] };
}

export function removeEntry(list: string[], entry: string): string[] {
  return list.filter((e) => e !== entry);
}
