import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  DEFAULT_SAMPLE,
  SAMPLE_SIZES,
  errorName,
  formatLatency,
  hasData,
  latencyRows,
  percent,
  type MetricsReport,
} from "./diagnosticsRules";
import { formatWhen } from "./format";
import { t, type StringKey } from "./i18n";
import { errorText } from "./useSettings";

const diagnosticsApi = {
  report: (limit: number) => invoke<MetricsReport>("metrics_report", { limit }),
  text: (limit: number) => invoke<string>("diagnostics_text", { limit }),
  exportLog: () => invoke<string>("export_log"),
  openLogDir: () => invoke<void>("open_log_dir"),
};

const ROW_LABEL: Record<"stt" | "llm" | "total", StringKey> = {
  stt: "diag.phase.stt",
  llm: "diag.phase.llm",
  total: "diag.phase.total",
};

export default function Diagnostics() {
  const [sample, setSample] = useState<number>(DEFAULT_SAMPLE);
  const [report, setReport] = useState<MetricsReport | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);

  const refresh = useCallback(() => {
    diagnosticsApi
      .report(sample)
      .then((r) => {
        setReport(r);
        setLoaded(true);
      })
      .catch((err: unknown) => {
        setLoaded(true);
        setNotice({ ok: false, text: errorText(err) });
      });
  }, [sample]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const copy = () => {
    diagnosticsApi
      .text(sample)
      .then((text) => navigator.clipboard.writeText(text))
      .then(() => setNotice({ ok: true, text: t("diag.copied") }))
      .catch((err: unknown) => setNotice({ ok: false, text: errorText(err) }));
  };
  const exportLog = () => {
    diagnosticsApi
      .exportLog()
      .then((path) => setNotice({ ok: true, text: t("diag.exported", { path }) }))
      .catch((err: unknown) => setNotice({ ok: false, text: errorText(err) }));
  };
  const openDir = () => {
    diagnosticsApi.openLogDir().catch((err: unknown) => setNotice({ ok: false, text: errorText(err) }));
  };

  return (
    <div className="settings">
      <section className="card">
        <h2>{t("diag.title")}</h2>
        <p className="hint small">{t("diag.lead")}</p>
        <div className="actions">
          <button type="button" className="primary" onClick={copy}>
            {t("diag.copy")}
          </button>
          <button type="button" onClick={exportLog}>
            {t("diag.export")}
          </button>
          <button type="button" onClick={openDir}>
            {t("diag.openDir")}
          </button>
        </div>
        <p className="hint small">{t("diag.privacy")}</p>
        {notice && (
          <p className={notice.ok ? "test-ok small" : "error small"} role="status">
            {notice.text}
          </p>
        )}
      </section>

      <section className="card">
        <h2>{t("diag.latency.title")}</h2>
        <div className="actions" role="group" aria-label={t("diag.sample")}>
          <span className="field-label">{t("diag.sample")}</span>
          {SAMPLE_SIZES.map((n) => (
            <button
              key={n}
              type="button"
              className={sample === n ? "primary" : ""}
              aria-pressed={sample === n}
              onClick={() => setSample(n)}
            >
              {n}
            </button>
          ))}
          <button type="button" onClick={refresh}>
            {t("diag.refresh")}
          </button>
        </div>

        {!loaded && <p className="hint">{t("common.loading")}</p>}
        {loaded && !hasData(report) && <p className="hint">{t("diag.empty")}</p>}
        {report && hasData(report) && (
          <>
            <p className="small">
              {t("diag.counts", {
                total: report.aggregate.total,
                ok: report.aggregate.ok,
                failed: report.aggregate.failed,
                okPct: percent(report.aggregate.ok, report.aggregate.total),
              })}
              {report.aggregate.llm_used > 0 &&
                ` ${t("diag.llmCounts", {
                  used: report.aggregate.llm_used,
                  fallbacks: report.aggregate.llm_fallbacks,
                })}`}
            </p>
            <table className="metrics">
              <thead>
                <tr>
                  <th>{t("diag.col.phase")}</th>
                  <th>p50</th>
                  <th>p95</th>
                  <th>{t("diag.col.max")}</th>
                  <th>{t("diag.col.samples")}</th>
                </tr>
              </thead>
              <tbody>
                {latencyRows(report.aggregate).map(({ key, summary }) => (
                  <tr key={key}>
                    <th scope="row">{t(ROW_LABEL[key])}</th>
                    <td>{formatLatency(summary.p50)}</td>
                    <td>{formatLatency(summary.p95)}</td>
                    <td>{formatLatency(summary.max)}</td>
                    <td>{summary.count}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        )}
      </section>

      {report && hasData(report) && (
        <section className="card">
          <h2>{t("diag.errors.title")}</h2>
          {report.aggregate.errors.length === 0 ? (
            <p className="hint">{t("diag.errors.none")}</p>
          ) : (
            <ul className="error-list">
              {report.aggregate.errors.map((e) => (
                <li key={e.code}>
                  <span>{errorName(e.code)}</span>
                  <span className="badge">{e.count}</span>
                </li>
              ))}
            </ul>
          )}
          {report.recent_errors.length > 0 && (
            <>
              <h3 className="small">{t("diag.errors.recent")}</h3>
              <ul className="error-list">
                {report.recent_errors.map((e, i) => (
                  <li key={`${e.created_at}-${i}`}>
                    <span>{errorName(e.code)}</span>
                    <span className="hint small">{formatWhen(e.created_at)}</span>
                  </li>
                ))}
              </ul>
            </>
          )}
        </section>
      )}
    </div>
  );
}
