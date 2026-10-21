// Teclas do Windows (virtual-key codes) para o gravador de atalho.
// Espelha src-tauri/src/hotkey/keys.rs: 0x0100 = "qualquer Win"; 0x11/0x10/0x12 = modificador de qualquer lado.

import { t, type StringKey } from "./i18n";

export const VK_ANY_WIN = 0x0100;
export const MAX_CHORD_KEYS = 3;

const FIXED: Record<string, number> = {
  ControlLeft: 0xa2,
  ControlRight: 0xa3,
  ShiftLeft: 0xa0,
  ShiftRight: 0xa1,
  AltLeft: 0xa4,
  AltRight: 0xa5,
  MetaLeft: 0x5b,
  MetaRight: 0x5c,
  CapsLock: 0x14,
  Space: 0x20,
  Tab: 0x09,
  Enter: 0x0d,
  Backspace: 0x08,
  Insert: 0x2d,
  Delete: 0x2e,
  Home: 0x24,
  End: 0x23,
  PageUp: 0x21,
  PageDown: 0x22,
  ArrowLeft: 0x25,
  ArrowUp: 0x26,
  ArrowRight: 0x27,
  ArrowDown: 0x28,
  Pause: 0x13,
  ScrollLock: 0x91,
  XButton1: 0x05,
  XButton2: 0x06,
};

/** `KeyboardEvent.code` → virtual-key code; `null` = tecla que o Vyrp não suporta em atalho. */
export function codeToVk(code: string): number | null {
  if (code in FIXED) return FIXED[code];
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1].charCodeAt(0);
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1].charCodeAt(0);
  const fn = /^F([0-9]{1,2})$/.exec(code);
  if (fn) {
    const n = Number(fn[1]);
    if (n >= 1 && n <= 24) return 0x70 + n - 1;
  }
  return null;
}

/** Botões extras do mouse (voltar/avançar) viram os pseudo-códigos XButton1/2. */
export function mouseButtonCode(button: number): string | null {
  if (button === 3) return "XButton1";
  if (button === 4) return "XButton2";
  return null;
}

const NAMES: Record<number, string> = {
  0x05: "Mouse 4",
  0x06: "Mouse 5",
  0x08: "Backspace",
  0x09: "Tab",
  0x0d: "Enter",
  0x10: "Shift",
  0x11: "Ctrl",
  0x12: "Alt",
  0x13: "Pause",
  0x14: "Caps Lock",
  0x1b: "Esc",
  0x21: "Page Up",
  0x22: "Page Down",
  0x23: "End",
  0x24: "Home",
  0x25: "←",
  0x26: "↑",
  0x27: "→",
  0x28: "↓",
  0x2d: "Insert",
  0x2e: "Delete",
  0x91: "Scroll Lock",
};

/** Nomes que mudam com o idioma (lados esquerdo/direito, espaço). */
const LOCALIZED: Record<number, StringKey> = {
  0x20: "key.space",
  0x5b: "key.lwin",
  0x5c: "key.rwin",
  0xa0: "key.lshift",
  0xa1: "key.rshift",
  0xa2: "key.lctrl",
  0xa3: "key.rctrl",
  0xa4: "key.lalt",
  0xa5: "key.ralt",
  [VK_ANY_WIN]: "key.win",
};

export function vkName(vk: number): string {
  if (vk in LOCALIZED) return t(LOCALIZED[vk]);
  if (vk in NAMES) return NAMES[vk];
  if ((vk >= 0x30 && vk <= 0x39) || (vk >= 0x41 && vk <= 0x5a)) return String.fromCharCode(vk);
  if (vk >= 0x70 && vk <= 0x87) return `F${vk - 0x70 + 1}`;
  return `0x${vk.toString(16).toUpperCase()}`;
}

export function chordLabel(chord: number[]): string {
  return chord.map(vkName).join(" + ");
}

export type RecordResult = { chord: number[] } | { error: string };

/**
 * Gravador de atalho, puro: recebe as teclas na ordem em que foram apertadas e
 * devolve o atalho ou um erro (no idioma da interface). Esc é reservado para cancelar.
 */
export function chordFromCodes(codes: string[]): RecordResult {
  const chord: number[] = [];
  for (const code of codes) {
    const vk = codeToVk(code);
    if (vk === null) return { error: t("key.err.unsupported", { code }) };
    if (!chord.includes(vk)) chord.push(vk);
  }
  if (chord.length === 0) return { error: t("key.err.none") };
  if (chord.length > MAX_CHORD_KEYS) {
    return { error: t("key.err.max", { max: MAX_CHORD_KEYS }) };
  }
  const hasModifier = chord.some(
    (vk) => [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0x5b, 0x5c, 0x05, 0x06].includes(vk),
  );
  if (!hasModifier) {
    return { error: t("key.err.modifier") };
  }
  return { chord };
}
