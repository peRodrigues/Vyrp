import { useCallback, useEffect, useState } from "react";
import { formatDuration, formatWhen, wordsLabel } from "./format";
import { t } from "./i18n";
import { historyApi } from "./settingsApi";
import type { HistoryItem, HistoryRetention } from "./settingsApi";
import { errorText } from "./useSettings";

const PAGE = 30;

function Row({ item, onDelete }: { item: HistoryItem; onDelete: (id: string) => void }) {
  const [raw, setRaw] = useState(false);
  const [copied, setCopied] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const shown = raw ? item.raw_text : item.final_text;
  const differs = item.raw_text !== item.final_text;

  const copy = () => {
    navigator.clipboard
      .writeText(shown)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      })
      .catch(() => undefined);
  };

  return (
    <li className="history-row">
      <div className="history-meta">
        <span>{formatWhen(item.created_at)}</span>
        <span>{item.app ?? t("history.unknownApp")}</span>
        <span>{formatDuration(item.duration_ms)}</span>
        <span>{wordsLabel(item.word_count)}</span>
        {differs && <span className="badge">{raw ? t("history.raw") : t("history.final")}</span>}
        {item.status === "inserted_unverified" && (
          <span className="badge" title={t("history.unverifiedHint")}>
            {t("status.inserted_unverified")}
          </span>
        )}
      </div>
      <p className="history-text">{shown}</p>
      <div className="row-actions">
        <button type="button" onClick={copy}>
          {copied ? t("common.copied") : t("common.copy")}
        </button>
        <button type="button" disabled title={t("history.repasteSoon")}>
          {t("history.repaste")}
        </button>
        {differs && (
          <button type="button" onClick={() => setRaw((r) => !r)}>
            {raw ? t("history.showFinal") : t("history.showRaw")}
          </button>
        )}
        {confirming ? (
          <span className="confirm">
            {t("common.delete")}?
            <button type="button" className="danger" onClick={() => onDelete(item.id)}>
              {t("common.yes")}
            </button>
            <button type="button" onClick={() => setConfirming(false)}>
              {t("common.no")}
            </button>
          </span>
        ) : (
          <button type="button" onClick={() => setConfirming(true)}>
            {t("common.delete")}
          </button>
        )}
      </div>
    </li>
  );
}

export default function History({ retention }: { retention: HistoryRetention | null }) {
  const [items, setItems] = useState<HistoryItem[]>([]);
  const [search, setSearch] = useState("");
  const [hasMore, setHasMore] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmingClear, setConfirmingClear] = useState(false);

  const load = useCallback(
    (offset: number) => {
      historyApi
        .list(search, PAGE, offset)
        .then((page) => {
          setItems((prev) => (offset === 0 ? page : [...prev, ...page]));
          setHasMore(page.length === PAGE);
        })
        .catch((err: unknown) => setNotice(errorText(err)));
    },
    [search],
  );

  useEffect(() => {
    load(0);
  }, [load]);

  const remove = (id: string) => {
    historyApi
      .remove(id)
      .then(() => setItems((prev) => prev.filter((i) => i.id !== id)))
      .catch((err: unknown) => setNotice(errorText(err)));
  };
  const clearAll = () => {
    historyApi
      .clear()
      .then(() => {
        setItems([]);
        setHasMore(false);
        setConfirmingClear(false);
      })
      .catch((err: unknown) => setNotice(errorText(err)));
  };

  return (
    <section className="history" aria-label={t("tab.history")}>
      <div className="toolbar">
        <input
          type="search"
          placeholder={t("history.search")}
          aria-label={t("history.search")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        {confirmingClear ? (
          <span className="confirm">
            {t("history.clearConfirm")}
            <button type="button" className="danger" onClick={clearAll}>
              {t("common.yes")}
            </button>
            <button type="button" onClick={() => setConfirmingClear(false)}>
              {t("common.no")}
            </button>
          </span>
        ) : (
          <button type="button" disabled={items.length === 0} onClick={() => setConfirmingClear(true)}>
            {t("history.clear")}
          </button>
        )}
      </div>

      {retention === "never" && <p className="notice">{t("history.off")}</p>}
      {notice && (
        <p className="notice" role="status" onClick={() => setNotice(null)}>
          {notice}
        </p>
      )}

      {items.length === 0 ? (
        <p className="empty">{search.trim() === "" ? t("history.empty") : t("history.emptySearch")}</p>
      ) : (
        <ul className="entries history-list">
          {items.map((item) => (
            <Row key={item.id} item={item} onDelete={remove} />
          ))}
        </ul>
      )}
      {hasMore && (
        <div className="more">
          <button type="button" onClick={() => load(items.length)}>
            {t("history.more")}
          </button>
        </div>
      )}
    </section>
  );
}
