import { describe, expect, it } from "vitest";
import {
  describeAction,
  formatDuration,
  formatSize,
  formatWhen,
  mergeRanking,
  moveItem,
  parsePhrases,
  wordsLabel,
} from "./format";
import { t } from "./i18n";

describe("formatWhen", () => {
  const now = new Date(2026, 9, 20, 15, 0);
  const at = (y: number, mo: number, d: number, h: number, mi: number) =>
    new Date(y, mo, d, h, mi).getTime();

  it("hoje, ontem e datas", () => {
    expect(formatWhen(at(2026, 9, 20, 14, 5), now)).toBe("hoje 14:05");
    expect(formatWhen(at(2026, 9, 19, 9, 10), now)).toBe("ontem 09:10");
    expect(formatWhen(at(2026, 9, 3, 8, 0), now)).toBe("03/10 08:00");
    expect(formatWhen(at(2025, 11, 31, 23, 59), now)).toBe("31/12/2025 23:59");
  });
});

describe("formatDuration", () => {
  it("segundos com vírgula e minutos", () => {
    expect(formatDuration(1800)).toBe("1,8 s");
    expect(formatDuration(0)).toBe("0,0 s");
    expect(formatDuration(65_000)).toBe("1 min 05 s");
  });
});

describe("wordsLabel", () => {
  it("singular e plural", () => {
    expect(wordsLabel(1)).toBe("1 palavra");
    expect(wordsLabel(12)).toBe("12 palavras");
    expect(wordsLabel(0)).toBe("0 palavras");
  });
});

describe("parsePhrases", () => {
  it("separa por vírgula, apara e descarta vazios", () => {
    expect(parsePhrases("nova linha, próxima linha ,, new line ")).toEqual([
      "nova linha",
      "próxima linha",
      "new line",
    ]);
    expect(parsePhrases("  ")).toEqual([]);
  });
});

describe("describeAction", () => {
  it("descreve cada ação em português", () => {
    expect(describeAction({ kind: "insert", text: "\n" })).toBe("Quebra de linha");
    expect(describeAction({ kind: "insert", text: "\n\n" })).toBe("Novo parágrafo");
    expect(describeAction({ kind: "insert", text: "—" })).toBe("Insere “—”");
    expect(describeAction({ kind: "undo_last" })).toBe("Desfaz o último texto colado");
  });
});

describe("formatSize", () => {
  it("MB e GB", () => {
    expect(formatSize(466)).toBe("466 MB");
    expect(formatSize(1500)).toBe("1,5 GB");
  });
});

describe("t", () => {
  it("troca variáveis e deixa as desconhecidas", () => {
    expect(t("general.mic.absent", { name: "USB" })).toBe("USB (desconectado)");
    expect(t("general.mic.absent")).toBe("{name} (desconectado)");
  });
});

describe("mergeRanking", () => {
  it("mantém a ordem salva, inclusive ausentes, e acrescenta os novos no fim", () => {
    expect(mergeRanking(["USB", "Interno"], ["Interno", "Bluetooth"])).toEqual([
      "USB",
      "Interno",
      "Bluetooth",
    ]);
    expect(mergeRanking([], ["A", "B"])).toEqual(["A", "B"]);
    expect(mergeRanking(["A", "A"], [])).toEqual(["A"]);
  });
});

describe("moveItem", () => {
  it("move para cima e para baixo sem alterar a original", () => {
    const list = ["a", "b", "c"];
    expect(moveItem(list, 2, 1)).toEqual(["a", "c", "b"]);
    expect(moveItem(list, 0, 1)).toEqual(["b", "a", "c"]);
    expect(list).toEqual(["a", "b", "c"]);
  });

  it("ignora movimentos inválidos", () => {
    const list = ["a", "b"];
    expect(moveItem(list, 0, -1)).toBe(list);
    expect(moveItem(list, 1, 2)).toBe(list);
    expect(moveItem(list, 1, 1)).toBe(list);
  });
});
