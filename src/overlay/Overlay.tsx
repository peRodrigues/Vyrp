import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  EVENT_LEVEL,
  EVENT_STATE,
  stateLabel,
  type AudioLevel,
  type DictationState,
} from "../shared/dictation";
import {
  HIDE_ONE_HOUR_MS,
  OVERLAY_PREF_KEY,
  hideFor,
  hideForever,
  isHidden,
  msUntilVisible,
  readPref,
  writePref,
  type HiddenPref,
} from "../shared/overlayPrefs";
import { t } from "../hub/i18n";
import { useUiLanguage } from "../shared/uiLanguage";
import { BARS, barHeights, pushLevel, silence } from "./waveform";

const BAR_MIN_PX = 3;
const BAR_MAX_PX = 22;
/** setTimeout aceita no máximo ~24,8 dias. */
const MAX_TIMER_MS = 2_147_483_647;

/** Preferência de ocultar; muda quando o hub escreve no localStorage ou o prazo vence. */
function useHiddenPref(): (pref: HiddenPref) => void {
  const [pref, setPref] = useState<HiddenPref>(readPref);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === OVERLAY_PREF_KEY || e.key === null) {
        setPref(readPref());
        setNow(Date.now());
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  // Volta sozinha quando o prazo de "ocultar por 1 h" vence.
  useEffect(() => {
    const wait = msUntilVisible(pref, Date.now());
    if (wait === null || wait === 0) return;
    const timer = window.setTimeout(() => setNow(Date.now()), Math.min(wait + 50, MAX_TIMER_MS));
    return () => window.clearTimeout(timer);
  }, [pref, now]);

  const hidden = isHidden(pref, now);
  useEffect(() => {
    const win = getCurrentWindow();
    void (hidden ? win.hide() : win.show());
  }, [hidden]);

  return (next: HiddenPref) => {
    writePref(next);
    setPref(next);
    setNow(Date.now());
  };
}

/** Texto dentro da pílula (a janela é estreita: travado usa a forma curta). */
function overlayLabel(state: DictationState): string {
  if (state.state === "idle") return t("overlay.rest");
  if (state.state === "listening" && state.locked) return t("overlay.locked");
  return stateLabel(state);
}

function Waveform({ levels }: { levels: number[] }) {
  const heights = barHeights(levels, BAR_MIN_PX, BAR_MAX_PX);
  return (
    <span className="wave" aria-hidden="true" data-tauri-drag-region>
      {heights.map((h, i) => (
        <span key={i} className="bar" style={{ height: h }} />
      ))}
    </span>
  );
}

function Icon({ state }: { state: DictationState }) {
  switch (state.state) {
    case "arming":
    case "processing":
    case "inserting":
      return <span className="spinner" aria-hidden="true" data-tauri-drag-region />;
    case "done":
      return (
        <span className="glyph" data-tauri-drag-region>
          ✓
        </span>
      );
    case "error":
      return (
        <span className="glyph" data-tauri-drag-region>
          !
        </span>
      );
    default:
      return <span className="dot" data-tauri-drag-region />;
  }
}

export default function Overlay() {
  const [state, setState] = useState<DictationState>({ state: "idle" });
  const [levels, setLevels] = useState<number[]>(() => silence());
  const [hover, setHover] = useState(false);
  const setHiddenPref = useHiddenPref();
  const language = useUiLanguage();

  useEffect(() => {
    const offState = listen<DictationState>(EVENT_STATE, (e) => {
      setState(e.payload);
      if (e.payload.state !== "listening") setLevels(silence());
    });
    const offLevel = listen<AudioLevel>(EVENT_LEVEL, (e) =>
      setLevels((prev) => pushLevel(prev, e.payload.rms, BARS)),
    );
    return () => {
      void offState.then((off) => off());
      void offLevel.then((off) => off());
    };
  }, []);

  const listening = state.state === "listening";
  const showHide = hover && state.state === "idle";
  const classes = ["pill", `pill--${state.state}`, listening && state.locked ? "pill--locked" : ""]
    .filter(Boolean)
    .join(" ");

  return (
    <div
      className={classes}
      lang={language}
      role="status"
      aria-live="polite"
      data-tauri-drag-region
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      <Icon state={state} />
      {listening && <Waveform levels={levels} />}
      {showHide ? (
        <span className="hide-actions">
          <button
            type="button"
            title={t("overlay.hide1hTitle")}
            onClick={() => setHiddenPref(hideFor(HIDE_ONE_HOUR_MS, Date.now()))}
          >
            {t("overlay.hide1h")}
          </button>
          <button
            type="button"
            title={t("overlay.hideForeverTitle")}
            onClick={() => setHiddenPref(hideForever())}
          >
            {t("overlay.hideForever")}
          </button>
        </span>
      ) : (
        <span className="label" data-tauri-drag-region>
          {overlayLabel(state)}
        </span>
      )}
    </div>
  );
}
