import { useState } from "react";
import { t } from "./i18n";
import { settingsApi, type ApiProvider } from "./settingsApi";
import { errorText } from "./useSettings";

interface Props {
  provider: ApiProvider;
  name: string;
  /** `null` = ainda carregando. */
  present: boolean | null;
  /** Chamado depois de salvar/remover, para quem mostra o status atualizar. */
  onChanged: () => void;
}

/** Uma chave de API: o valor só entra (campo de senha, limpo ao salvar) e nunca volta. */
export default function ApiKeyRow({ provider, name, present, onChanged }: Props) {
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  const save = () => {
    settingsApi
      .setKey(provider, value)
      .then(() => {
        setValue("");
        setError(null);
        onChanged();
      })
      .catch((err: unknown) => setError(errorText(err)));
  };
  const clear = () => {
    settingsApi
      .clearKey(provider)
      .then(() => {
        setError(null);
        onChanged();
      })
      .catch((err: unknown) => setError(errorText(err)));
  };

  return (
    <div className="key-row">
      <strong className="key-name">{name}</strong>
      <span className={present ? "badge snippet" : "badge"}>
        {present === null ? "…" : present ? t("advanced.keys.set") : t("advanced.keys.unset")}
      </span>
      <input
        type="password"
        autoComplete="off"
        aria-label={t("advanced.keys.aria", { name })}
        placeholder={t("advanced.keys.placeholder")}
        value={value}
        onChange={(e) => setValue(e.target.value)}
      />
      <button type="button" className="primary" disabled={value.trim() === ""} onClick={save}>
        {t("common.save")}
      </button>
      <button type="button" disabled={!present} onClick={clear}>
        {t("common.remove")}
      </button>
      {error && (
        <p className="error key-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
