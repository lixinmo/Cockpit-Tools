import { useCallback, useEffect, useId, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { ModalErrorMessage } from "../ModalErrorMessage";
import "./CodexAutoConsumeResetSettings.css";

interface AutoConsumeResetConfig {
  enabled: boolean;
  threshold: number;
}

interface EditorState {
  accountId: string;
  config: AutoConsumeResetConfig | null;
  threshold: string;
  loading: boolean;
  saving: boolean;
  error: string | null;
}

const SETTINGS_CHANGED = "codex-auto-reset-settings-updated";
const emptyState = (accountId: string): EditorState => ({
  accountId, config: null, threshold: "2", loading: true, saving: false, error: null,
});

export function CodexAutoConsumeResetSettings({ accountId, disabled = false }: { accountId: string; disabled?: boolean }) {
  const { t } = useTranslation();
  const thresholdId = useId();
  const descriptionId = useId();
  const [state, setState] = useState(() => emptyState(accountId));
  const currentAccountId = useRef(accountId);
  currentAccountId.current = accountId;
  const requestSequence = useRef(0);
  const mounted = useRef(false);
  const savesInFlight = useRef(new Set<string>());
  // A prop change must never display or submit the previous account's settings,
  // including the render before effects run.
  const { config, threshold, loading, saving: stateSaving, error } = state.accountId === accountId
    ? state : emptyState(accountId);
  const saving = stateSaving || savesInFlight.current.has(accountId);

  const updateState = (updates: Partial<EditorState>) => {
    if (!mounted.current || currentAccountId.current !== accountId) return;
    setState(current => current.accountId === accountId ? { ...current, ...updates } : current);
  };

  const loadConfig = useCallback(async () => {
    if (!mounted.current || currentAccountId.current !== accountId) return;
    const request = ++requestSequence.current;
    setState({ ...emptyState(accountId), loading: !!accountId });
    if (!accountId) return;
    const isCurrent = () => mounted.current && currentAccountId.current === accountId
      && requestSequence.current === request;
    try {
      const loaded = await invoke<AutoConsumeResetConfig>("get_codex_auto_reset_settings", { accountId });
      if (!isCurrent()) return;
      setState({ ...emptyState(accountId), config: loaded, threshold: String(loaded.threshold), loading: false });
    } catch (err) {
      if (!isCurrent()) return;
      setState({ ...emptyState(accountId), loading: false,
        error: t("quickSettings.error.loadFailed", { error: String(err) }) });
    }
  }, [accountId, t]);

  useEffect(() => {
    mounted.current = true;
    void loadConfig();
    const reload = (event: Event) => {
      if ((event as CustomEvent<{ accountId: string }>).detail?.accountId === accountId
        && !savesInFlight.current.has(accountId)) void loadConfig();
    };
    window.addEventListener(SETTINGS_CHANGED, reload);
    return () => {
      mounted.current = false;
      ++requestSequence.current;
      window.removeEventListener(SETTINGS_CHANGED, reload);
    };
  }, [accountId, loadConfig]);

  const parsedThreshold = Number(threshold);
  const thresholdValid = threshold.trim() !== ""
    && Number.isInteger(parsedThreshold)
    && parsedThreshold >= 0
    && parsedThreshold <= 99;

  const saveConfig = async (updates: Partial<AutoConsumeResetConfig>) => {
    if (!accountId || !mounted.current || currentAccountId.current !== accountId
      || !config || loading || savesInFlight.current.has(accountId) || disabled) return;
    const request = requestSequence.current;
    const isCurrent = () => mounted.current && currentAccountId.current === accountId
      && requestSequence.current === request;
    savesInFlight.current.add(accountId);
    updateState({ saving: true, error: null });
    try {
      const updated = await invoke<AutoConsumeResetConfig>("set_codex_auto_reset_settings", { accountId, ...updates });
      if (isCurrent()) {
        updateState({ config: updated, threshold: String(updated.threshold) });
      } else if (mounted.current && currentAccountId.current === accountId) {
        // A -> B -> A while A was saving: read the persisted result anew.
        void loadConfig();
      }
      window.dispatchEvent(new CustomEvent(SETTINGS_CHANGED, { detail: { accountId } }));
    } catch (err) {
      if (isCurrent()) updateState({ error: t("quickSettings.error.saveFailed", { error: String(err) }) });
    } finally {
      savesInFlight.current.delete(accountId);
      updateState({ saving: false });
    }
  };

  return (
    <section className="codex-auto-consume-reset" aria-busy={loading || saving}>
      <div className="codex-auto-consume-reset-row">
        <label className="codex-auto-consume-reset-toggle">
          <span>Auto Consume Reset</span>
          <span className="qs-switch">
            <input
              type="checkbox"
              checked={config?.enabled ?? false}
              disabled={!config || loading || saving || disabled}
              aria-describedby={descriptionId}
              onChange={(event) => {
                if (event.target.checked && !thresholdValid) {
                  updateState({ error: t("codex.quota.autoConsumeResetInvalidThreshold") });
                  return;
                }
                void saveConfig({
                  enabled: event.target.checked,
                  ...(event.target.checked ? { threshold: parsedThreshold } : {}),
                });
              }}
            />
            <span className="qs-switch-slider" />
          </span>
        </label>
      </div>
      <p id={descriptionId} className="codex-auto-consume-reset-description">
        {t("codex.quota.autoConsumeResetDescription")}
      </p>
      <form
        className="codex-auto-consume-reset-row"
        onSubmit={(event) => {
          event.preventDefault();
          if (!thresholdValid) {
            updateState({ error: t("codex.quota.autoConsumeResetInvalidThreshold") });
            return;
          }
          void saveConfig({ threshold: parsedThreshold });
        }}
      >
        <label htmlFor={thresholdId}>{t("codex.quota.autoConsumeResetThreshold")}</label>
        <div className="codex-auto-consume-reset-threshold">
          <input
            id={thresholdId}
            type="number"
            min={0}
            max={99}
            step={1}
            required
            value={threshold}
            disabled={!config || loading || saving || disabled}
            onChange={(event) => updateState({ threshold: event.target.value })}
          />
          <span>%</span>
          <button
            type="submit"
            className="btn btn-secondary compact"
            disabled={!config || loading || saving || disabled || !thresholdValid
              || parsedThreshold === config.threshold}
          >
            {t("common.save")}
          </button>
        </div>
      </form>
      <ModalErrorMessage message={error} position="bottom" />
      {!config && !loading && (
        <button type="button" className="btn btn-secondary compact" onClick={() => void loadConfig()}>
          {t("common.refresh")}
        </button>
      )}
    </section>
  );
}
