import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  EVENT_MODEL_DONE,
  EVENT_MODEL_ERROR,
  EVENT_MODEL_PROGRESS,
  EVENT_STT_STATUS,
  modelsApi,
  type DownloadProgress,
  type DownloadResult,
  type ModelEntry,
  type SttStatus,
} from "./modelsApi";
import { applyProgress, withoutKey, type ProgressMap } from "./onboardingState";
import { SETTINGS_CHANGED } from "./settingsApi";
import { withTimeout } from "./timeout";
import { rawError, toUiError, type UiError } from "./uiError";

export interface ModelsState {
  models: ModelEntry[];
  progress: ProgressMap;
  errors: Record<string, UiError>;
  status: SttStatus | null;
  /** A lista não veio (erro ou prazo esgotado): a tela mostra o erro e "Tentar de novo". */
  listFailed: boolean;
  retry: () => void;
  download: (id: string) => void;
  select: (id: string) => void;
}

/** Lista de modelos + progresso dos downloads + estado do motor, tudo pelos eventos da Forja. */
export function useModels(): ModelsState {
  const [models, setModels] = useState<ModelEntry[]>([]);
  const [progress, setProgress] = useState<ProgressMap>({});
  const [errors, setErrors] = useState<Record<string, UiError>>({});
  const [status, setStatus] = useState<SttStatus | null>(null);
  const [listFailed, setListFailed] = useState(false);

  const refresh = useCallback(() => {
    withTimeout(modelsApi.list())
      .then((list) => {
        setModels(list);
        setListFailed(false);
      })
      .catch(() => setListFailed(true));
  }, []);

  const retry = useCallback(() => {
    setListFailed(false);
    refresh();
  }, [refresh]);

  useEffect(() => {
    refresh();
    modelsApi
      .status()
      .then(setStatus)
      .catch(() => setStatus(null));
    const subscriptions = [
      listen<DownloadProgress>(EVENT_MODEL_PROGRESS, (e) =>
        setProgress((prev) => applyProgress(prev, e.payload)),
      ),
      listen<DownloadResult>(EVENT_MODEL_DONE, (e) => {
        setProgress((prev) => withoutKey(prev, e.payload.id));
        setErrors((prev) => withoutKey(prev, e.payload.id));
        refresh();
      }),
      listen<DownloadResult>(EVENT_MODEL_ERROR, (e) => {
        setProgress((prev) => withoutKey(prev, e.payload.id));
        setErrors((prev) => ({ ...prev, [e.payload.id]: e.payload.error ?? rawError("") }));
        refresh();
      }),
      listen<SttStatus>(EVENT_STT_STATUS, (e) => setStatus(e.payload)),
      listen(SETTINGS_CHANGED, refresh),
    ];
    return () => {
      for (const sub of subscriptions) void sub.then((off) => off());
    };
  }, [refresh]);

  const download = useCallback(
    (id: string) => {
      setErrors((prev) => withoutKey(prev, id));
      setProgress((prev) => applyProgress(prev, { id, downloaded: 0, total: 0 }));
      modelsApi.download(id).catch((err: unknown) => {
        setProgress((prev) => withoutKey(prev, id));
        setErrors((prev) => ({ ...prev, [id]: toUiError(err) }));
      });
    },
    [],
  );

  const select = useCallback(
    (id: string) => {
      modelsApi
        .select(id)
        .then(setStatus)
        .then(refresh)
        .catch((err: unknown) => setErrors((prev) => ({ ...prev, [id]: toUiError(err) })));
    },
    [refresh],
  );

  return { models, progress, errors, status, listFailed, retry, download, select };
}
