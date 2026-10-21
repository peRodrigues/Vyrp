import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { t } from "./i18n";
import { uiErrorText } from "./uiError";
import type { SettingsState } from "./useSettings";
import ShortcutRecorder from "./ShortcutRecorder";
import { transformApi, type SavedPrompt, type Transformation, type TransformStatus } from "./transformApi";
import { selectionDiff, setTransformChord, transformChord } from "./transformRules";
import "./transforms.css";

function Diff({ row }: { row: Transformation }) {
  const d = selectionDiff(row.original_text, row.transformed_text);
  return <div className="transform-diff">
    <div><strong>{t("transform.before")}</strong><pre>{d.prefix}<del>{d.removed}</del>{d.suffix}</pre></div>
    <div><strong>{t("transform.after")}</strong><pre>{d.prefix}<ins>{d.added}</ins>{d.suffix}</pre></div>
  </div>;
}

export default function Transforms({ settings, update, error: settingsError }: SettingsState) {
  const [prompts, setPrompts] = useState<SavedPrompt[]>([]);
  const [rows, setRows] = useState<Transformation[]>([]);
  const [offset, setOffset] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<TransformStatus>({ phase: "idle", error: null, entry_id: null });
  const refresh = useCallback(() => transformApi.history(offset).then(setRows), [offset]);
  useEffect(() => { void transformApi.prompts().then(setPrompts).catch(e => setError(uiErrorText(e))); }, []);
  useEffect(() => { void refresh().catch(e => setError(uiErrorText(e))); }, [refresh, settings?.history_retention]);
  useEffect(() => {
    void transformApi.status().then(setStatus).catch(e => setError(uiErrorText(e)));
    const off = listen<TransformStatus>("transform://state", ({ payload }) => {
      setStatus(payload);
      if (payload.phase === "idle") void refresh().catch(e => setError(uiErrorText(e)));
    });
    return () => { void off.then(fn => fn()); };
  }, [refresh]);
  if (!settings) return null;
  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true); setError(null);
    try { await fn(); await refresh(); } catch (e) { setError(uiErrorText(e)); } finally { setBusy(false); }
  };
  const shortcut = (slot: number | null, chord: number[] | null) => update({ gestures: { ...settings.gestures, actions: setTransformChord(settings.gestures.actions, slot, chord) } });
  const edit = (slot: number, patch: Partial<SavedPrompt>) => { setSaved(null); setPrompts(old => old.map(p => p.slot === slot ? { ...p, ...patch } : p)); };
  return <>
    <section className="card">
      <h2>{t("transform.title")}</h2>
      <p>{t("transform.help")}</p>
      <p className="hint">{t("transform.privacy")}</p>
      {!settings.llm_provider && <p role="alert">{t("uierr.transform.provider")}</p>}
      <ShortcutRecorder label={t("transform.voice")} value={transformChord(settings.gestures.actions, null)} onChange={c => shortcut(null, c)} onClear={() => shortcut(null, null)} />
      <p role="status">{t(`transform.phase.${status.phase}`)}</p>
      {status.phase !== "idle" && <button type="button" onClick={() => void run(transformApi.cancel)}>{t("transform.cancel")}</button>}
      {Boolean(error || settingsError || status.error) && <p role="alert" className="error">{error || settingsError || uiErrorText(status.error)}</p>}
    </section>
    <section className="card">
      <h2>{t("transform.prompts")}</h2>
      {prompts.map(p => <details key={p.slot} className="transform-prompt">
        <summary>{t("transform.slot", { slot: p.slot })}{p.title ? ` — ${p.title}` : ""}</summary>
        <label className="field">{t("transform.name")}<input maxLength={100} value={p.title} onChange={e => edit(p.slot, { title: e.target.value })} /></label>
        <label className="field">{t("transform.instruction")}<textarea rows={3} maxLength={2048} value={p.instruction} onChange={e => edit(p.slot, { instruction: e.target.value })} /></label>
        <ShortcutRecorder label={t("transform.shortcut")} value={transformChord(settings.gestures.actions, p.slot)} onChange={c => shortcut(p.slot, c)} onClear={() => shortcut(p.slot, null)} />
        <button type="button" disabled={busy} onClick={() => void run(async () => { await transformApi.save(p); setSaved(p.slot); })}>{t("transform.save")}</button>
        {saved === p.slot && <span role="status">{t("transform.saved")}</span>}
      </details>)}
    </section>
    <section className="card">
      <h2>{t("transform.history")}</h2>
      <p className="hint">{t("transform.undoHelp")}</p>
      <button type="button" disabled={busy} onClick={() => void run(refresh)}>{t("transform.refresh")}</button>
      <button type="button" disabled={busy || rows.length === 0} onClick={() => { if (window.confirm(t("transform.clearConfirm"))) void run(() => transformApi.delete(null)); }}>{t("transform.clear")}</button>
      {rows.length === 0 && <p>{t("transform.empty")}</p>}
      {rows.map(row => <article key={row.id} className="transform-entry">
        <p><time>{new Date(row.created_at).toLocaleString()}</time>{" · "}{row.provider}{" · "}{t(`transform.status.${row.status}`)}</p>
        <p>{row.instruction}</p><Diff row={row} />
        <button type="button" disabled={busy || status.phase !== "idle" || row.status !== "inserted"} onClick={() => void run(() => transformApi.undo(row.id))}>{t("transform.undo")}</button>
        <button type="button" disabled={busy} onClick={() => void run(() => transformApi.delete(row.id))}>{t("transform.delete")}</button>
      </article>)}
      <div className="actions">
        <button type="button" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - 30))}>{t("transform.previous")}</button>
        <button type="button" disabled={rows.length < 30} onClick={() => setOffset(offset + 30)}>{t("transform.next")}</button>
      </div>
    </section>
  </>;
}
