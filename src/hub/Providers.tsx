import { useState } from "react";
import ApiKeyRow from "./ApiKeyRow";
import { t, type StringKey } from "./i18n";
import {
  CLEANUP_LEVELS,
  LLM_DEFAULT_MODEL,
  LLM_PROVIDERS,
  LLM_DEFAULT_URL,
  PROVIDER_NAMES,
  STT_CLOUDS,
  STT_POLICIES,
  WRITING_STYLES,
  effectiveModel,
  missingKeys,
  normalizeModel,
  normalizeUrl,
  supportsCustomUrl,
  sttUsesCloud,
  type KeyWarning,
  type LlmProviderId,
  type ProviderTarget,
  type ProviderTestResult,
  type SttCloud,
} from "./providerRules";
import { providersApi, type ApiProvider } from "./settingsApi";
import { toUiError, uiErrorText } from "./uiError";
import type { SettingsState } from "./useSettings";
import { useKeyStatus } from "./useKeyStatus";

const KEY_ROWS: ApiProvider[] = ["groq", "open_ai", "anthropic"];

const POLICY_LABEL: Record<(typeof STT_POLICIES)[number], StringKey> = {
  local_only: "prov.policy.local_only",
  cloud_only: "prov.policy.cloud_only",
  cloud_with_fallback: "prov.policy.cloud_with_fallback",
};
const POLICY_HINT: Record<(typeof STT_POLICIES)[number], StringKey> = {
  local_only: "prov.policy.local_only.hint",
  cloud_only: "prov.policy.cloud_only.hint",
  cloud_with_fallback: "prov.policy.cloud_with_fallback.hint",
};
const LEVEL_LABEL: Record<(typeof CLEANUP_LEVELS)[number], StringKey> = {
  none: "prov.level.none",
  light: "prov.level.light",
  medium: "prov.level.medium",
  high: "prov.level.high",
};
const LEVEL_EXAMPLE: Record<(typeof CLEANUP_LEVELS)[number], StringKey> = {
  none: "prov.level.none.example",
  light: "prov.level.light.example",
  medium: "prov.level.medium.example",
  high: "prov.level.high.example",
};
const STYLE_LABEL: Record<(typeof WRITING_STYLES)[number], StringKey> = {
  formal: "prov.style.formal",
  casual: "prov.style.casual",
  very_casual: "prov.style.very_casual",
};
const STYLE_EXAMPLE: Record<(typeof WRITING_STYLES)[number], StringKey> = {
  formal: "prov.style.formal.example",
  casual: "prov.style.casual.example",
  very_casual: "prov.style.very_casual.example",
};

function Warning({ warning, onFix }: { warning: KeyWarning; onFix: () => void }) {
  const key: StringKey = warning.scope === "stt" ? "prov.warn.stt" : "prov.warn.llm";
  return (
    <p className="warning small" role="alert">
      {t(key, { provider: PROVIDER_NAMES[warning.provider] })}{" "}
      <button type="button" className="link" onClick={onFix}>
        {t("prov.warn.fix")}
      </button>
    </p>
  );
}

/** Botão "Testar": chama o provedor com um texto/áudio curto e mostra ok/erro (sem expor chave). */
function TestButton({ target, disabled }: { target: ProviderTarget; disabled?: boolean }) {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ProviderTestResult | null>(null);

  const run = () => {
    setBusy(true);
    setResult(null);
    providersApi
      .test(target)
      .then(setResult)
      .catch((err: unknown) => setResult({ ok: false, message: toUiError(err), ms: 0 }))
      .finally(() => setBusy(false));
  };

  return (
    <div className="test-row">
      <button type="button" disabled={disabled || busy} onClick={run}>
        {busy ? t("prov.testing") : t("prov.test")}
      </button>
      {result && (
        <span className={result.ok ? "test-ok small" : "error small"} role="status">
          {result.ok ? "✓ " : "✗ "}
          {uiErrorText(result.message)}
          {result.ms > 0 && ` (${result.ms} ms)`}
        </span>
      )}
    </div>
  );
}

function ModelField({ provider, value, onCommit }: { provider: LlmProviderId; value: string | null; onCommit: (m: string | null) => void }) {
  // O `key` recria o campo quando o valor salvo muda (ex.: troca de provedor).
  return <ModelInput key={`${provider}:${value ?? ""}`} provider={provider} value={value} onCommit={onCommit} />;
}

function ModelInput({ provider, value, onCommit }: { provider: LlmProviderId; value: string | null; onCommit: (m: string | null) => void }) {
  const [draft, setDraft] = useState(value ?? "");
  const commit = () => {
    const next = normalizeModel(draft);
    if (next !== value) onCommit(next);
  };
  return (
    <label className="field">
      <span className="field-label">{t("prov.llm.model")}</span>
      <input
        value={draft}
        placeholder={LLM_DEFAULT_MODEL[provider]}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
        }}
      />
      <span className="hint small">
        {t("prov.llm.modelUsing", { model: effectiveModel(provider, value) })}
      </span>
    </label>
  );
}

function UrlField({ provider, value, onCommit }: { provider: LlmProviderId; value: string | null; onCommit: (u: string | null) => void }) {
  return <UrlInput key={`${provider}:${value ?? ""}`} provider={provider} value={value} onCommit={onCommit} />;
}

function UrlInput({ provider, value, onCommit }: { provider: LlmProviderId; value: string | null; onCommit: (u: string | null) => void }) {
  const [draft, setDraft] = useState(value ?? "");
  const commit = () => {
    const next = normalizeUrl(draft);
    if (next !== value) onCommit(next);
  };
  const hintKey: StringKey = provider === "ollama" ? "prov.llm.url.ollama" : "prov.llm.url.open_ai";
  return (
    <label className="field">
      <span className="field-label">{t("prov.llm.url")}</span>
      <input
        value={draft}
        placeholder={LLM_DEFAULT_URL[provider]}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
        }}
      />
      <span className="hint small">{t(hintKey, { url: LLM_DEFAULT_URL[provider] })}</span>
    </label>
  );
}

export default function Providers({ settings, error, update }: SettingsState) {
  const { status, refresh } = useKeyStatus();
  if (!settings) return <p className="hint">{error ?? t("common.loading")}</p>;

  const warnings = missingKeys(settings, status);
  const warningFor = (scope: KeyWarning["scope"]) => warnings.find((w) => w.scope === scope);
  const scrollToKeys = () => document.getElementById("api-keys")?.scrollIntoView({ behavior: "smooth" });
  const llm = settings.llm_provider;

  return (
    <div className="settings">
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <section className="card">
        <h2>{t("prov.stt.title")}</h2>
        <p className="hint small">{t("prov.stt.lead")}</p>
        <div className="radios" role="radiogroup" aria-label={t("prov.stt.title")}>
          {STT_POLICIES.map((policy) => (
            <label key={policy} className="check radio-block">
              <input
                type="radio"
                name="stt-policy"
                checked={settings.stt_policy === policy}
                onChange={() => update({ stt_policy: policy })}
              />
              <span>
                {t(POLICY_LABEL[policy])}
                <span className="hint small block">{t(POLICY_HINT[policy])}</span>
              </span>
            </label>
          ))}
        </div>
        {sttUsesCloud(settings.stt_policy) && (
          <>
            <label className="field">
              <span className="field-label">{t("prov.stt.provider")}</span>
              <select
                value={settings.stt_cloud}
                onChange={(e) => update({ stt_cloud: e.target.value as SttCloud })}
              >
                {STT_CLOUDS.map((id) => (
                  <option key={id} value={id}>
                    {PROVIDER_NAMES[id]}
                  </option>
                ))}
              </select>
            </label>
            {warningFor("stt") && <Warning warning={warningFor("stt")!} onFix={scrollToKeys} />}
            <TestButton target={{ kind: "stt_cloud", provider: settings.stt_cloud }} />
            <p className="hint small">{t("prov.stt.privacy", { provider: PROVIDER_NAMES[settings.stt_cloud] })}</p>
          </>
        )}
      </section>

      <section className="card">
        <h2>{t("prov.llm.title")}</h2>
        <p className="hint small">{t("prov.llm.lead")}</p>
        <label className="field">
          <span className="field-label">{t("prov.llm.provider")}</span>
          <select
            value={llm ?? ""}
            onChange={(e) =>
              update({
                llm_provider: e.target.value === "" ? null : (e.target.value as LlmProviderId),
                llm_model: null,
                llm_url: null,
              })
            }
          >
            <option value="">{t("prov.llm.off")}</option>
            {LLM_PROVIDERS.map((id) => (
              <option key={id} value={id}>
                {PROVIDER_NAMES[id]}
              </option>
            ))}
          </select>
        </label>
        {llm && (
          <>
            <ModelField provider={llm} value={settings.llm_model} onCommit={(m) => update({ llm_model: m })} />
            {supportsCustomUrl(llm) && (
              <UrlField provider={llm} value={settings.llm_url} onCommit={(u) => update({ llm_url: u })} />
            )}
            {warningFor("llm") && <Warning warning={warningFor("llm")!} onFix={scrollToKeys} />}
            <TestButton target={{ kind: "llm", provider: llm }} />
          </>
        )}

        <label className="field">
          <span className="field-label">{t("prov.level.title")}</span>
          <select
            value={settings.cleanup}
            onChange={(e) => update({ cleanup: e.target.value as (typeof CLEANUP_LEVELS)[number] })}
          >
            {CLEANUP_LEVELS.map((level) => (
              <option key={level} value={level}>
                {t(LEVEL_LABEL[level])}
              </option>
            ))}
          </select>
        </label>
        <p className="example">{t(LEVEL_EXAMPLE[settings.cleanup])}</p>
        {!llm && settings.cleanup !== "none" && <p className="hint small">{t("prov.level.needsLlm")}</p>}

        <label className="field">
          <span className="field-label">{t("prov.style.title")}</span>
          <select
            value={settings.style}
            onChange={(e) => update({ style: e.target.value as (typeof WRITING_STYLES)[number] })}
          >
            {WRITING_STYLES.map((style) => (
              <option key={style} value={style}>
                {t(STYLE_LABEL[style])}
              </option>
            ))}
          </select>
        </label>
        <p className="example">{t(STYLE_EXAMPLE[settings.style])}</p>
      </section>

      <section className="card" id="api-keys">
        <h2>{t("advanced.keys.title")}</h2>
        <p className="hint small">{t("advanced.keys.hint")}</p>
        {KEY_ROWS.map((id) => (
          <ApiKeyRow
            key={id}
            provider={id}
            name={PROVIDER_NAMES[id]}
            present={status[id] ?? null}
            onChanged={refresh}
          />
        ))}
      </section>
    </div>
  );
}
