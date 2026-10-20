import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  EVENT_LEVEL,
  EVENT_STATE,
  stateLabel,
  type AudioLevel,
  type DictationState,
} from "../shared/dictation";

export default function Overlay() {
  const [state, setState] = useState<DictationState>({ state: "idle" });
  const [level, setLevel] = useState(0);

  useEffect(() => {
    const offState = listen<DictationState>(EVENT_STATE, (e) => setState(e.payload));
    const offLevel = listen<AudioLevel>(EVENT_LEVEL, (e) => setLevel(e.payload.rms));
    return () => {
      offState.then((off) => off());
      offLevel.then((off) => off());
    };
  }, []);

  return (
    <div className={`pill pill--${state.state}`} role="status" aria-live="polite">
      <span className="dot" style={{ transform: `scale(${1 + Math.min(level, 1)})` }} />
      <span>{stateLabel(state)}</span>
    </div>
  );
}
