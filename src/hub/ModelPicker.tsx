import { formatBytes } from "./format";
import { t } from "./i18n";
import { modelName } from "./modelsApi";
import { uiErrorText } from "./uiError";
import { downloadPercent } from "./onboardingState";
import type { ModelsState } from "./useModels";

function statusLine(status: ModelsState["status"]): string | null {
  if (!status) return null;
  if (status.state === "loading") return t("model.status.loading");
  if (status.state === "ready") return t("model.status.ready", { backend: status.backend });
  if (status.state === "error") return t("model.status.error", { error: status.error ? uiErrorText(status.error) : "?" });
  return null;
}

/** Lista de modelos locais: baixar com progresso, escolher o ativo. Usada no onboarding e em Geral. */
export default function ModelPicker({
  models,
  progress,
  errors,
  status,
  listFailed,
  retry,
  download,
  select,
}: ModelsState) {
  const line = statusLine(status);
  return (
    <div className="model-picker">
      {models.length === 0 &&
        (listFailed ? (
          <p className="error small" role="alert">
            {t("model.listFailed")}{" "}
            <button type="button" onClick={retry}>
              {t("common.retry")}
            </button>
          </p>
        ) : (
          <p className="hint small">{t("common.loading")}</p>
        ))}
      <ul className="models">
        {models.map((m) => {
          const downloading = m.downloading || m.id in progress;
          const pct = downloadPercent(progress[m.id]);
          const error = errors[m.id];
          return (
            <li key={m.id} className={m.active ? "model active" : "model"}>
              <div className="entry-text">
                <strong>{modelName(m.id)}</strong>
                {m.recommended && <span className="badge snippet">{t("model.recommended")}</span>}
                {m.active && m.installed && <span className="badge">{t("model.active")}</span>}
                {downloading && (
                  <div className="progress" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
                    <span style={{ width: `${pct}%` }} />
                    <em>
                      {pct > 0
                        ? t("model.progress", {
                            pct,
                            done: formatBytes(progress[m.id]?.downloaded ?? 0),
                            total: formatBytes(progress[m.id]?.total ?? m.size_bytes),
                          })
                        : t("model.starting")}
                    </em>
                  </div>
                )}
                {error && (
                  <p className="error small" role="alert">
                    {t("model.downloadFailed", { error: uiErrorText(error) })}
                  </p>
                )}
              </div>
              {m.installed ? (
                m.active ? null : (
                  <button type="button" onClick={() => select(m.id)}>
                    {t("model.use")}
                  </button>
                )
              ) : (
                <button type="button" className="primary" disabled={downloading} onClick={() => download(m.id)}>
                  {downloading ? t("model.downloading") : error ? t("model.retry") : t("model.download")}
                </button>
              )}
            </li>
          );
        })}
      </ul>
      {line && <p className="hint small">{line}</p>}
    </div>
  );
}
