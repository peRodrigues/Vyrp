import { useEffect, useRef, useState } from "react";
import { chordFromCodes, chordLabel, mouseButtonCode } from "./keys";
import { t } from "./i18n";

interface Props {
  label: string;
  /** Atalho atual; `null` = sem atalho. */
  value: number[] | null;
  onChange: (chord: number[]) => void;
  /** Se informado, mostra "Remover" quando houver atalho. */
  onClear?: () => void;
  emptyText?: string;
}

/**
 * Grava um atalho: o usuário aperta as teclas juntas e solta. Esc cancela a gravação.
 * Vale só para teclas e botões extras do mouse que o gancho global do Vyrp reconhece.
 */
export default function ShortcutRecorder({ label, value, onChange, onClear, emptyText }: Props) {
  const [recording, setRecording] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const onChangeRef = useRef(onChange);
  useEffect(() => {
    onChangeRef.current = onChange;
  }, [onChange]);

  useEffect(() => {
    if (!recording) return;
    const order: string[] = [];
    const down = new Set<string>();

    const press = (code: string) => {
      if (!down.has(code)) {
        down.add(code);
        order.push(code);
      }
    };
    const release = (code: string) => {
      down.delete(code);
      if (down.size > 0 || order.length === 0) return;
      setRecording(false);
      const result = chordFromCodes(order);
      if ("chord" in result) {
        setError(null);
        onChangeRef.current(result.chord);
      } else {
        setError(result.error);
      }
    };

    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.code === "Escape") {
        setRecording(false);
        return;
      }
      press(e.code);
    };
    const onKeyUp = (e: KeyboardEvent) => {
      e.preventDefault();
      release(e.code);
    };
    const onMouseDown = (e: MouseEvent) => {
      const code = mouseButtonCode(e.button);
      if (!code) return;
      e.preventDefault();
      press(code);
    };
    const onMouseUp = (e: MouseEvent) => {
      const code = mouseButtonCode(e.button);
      if (!code) return;
      e.preventDefault();
      release(code);
    };

    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("mousedown", onMouseDown, true);
    window.addEventListener("mouseup", onMouseUp, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("mousedown", onMouseDown, true);
      window.removeEventListener("mouseup", onMouseUp, true);
    };
  }, [recording]);

  return (
    <div className="field">
      <span className="field-label">{label}</span>
      <div className="recorder">
        <kbd className={recording ? "chord recording" : "chord"}>
          {recording ? t("general.keys.recording") : value ? chordLabel(value) : (emptyText ?? "—")}
        </kbd>
        <button
          type="button"
          onClick={() => {
            setError(null);
            setRecording((r) => !r);
          }}
        >
          {recording ? t("common.cancel") : t("general.keys.record")}
        </button>
        {onClear && value && !recording && (
          <button type="button" onClick={onClear}>
            {t("common.remove")}
          </button>
        )}
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
