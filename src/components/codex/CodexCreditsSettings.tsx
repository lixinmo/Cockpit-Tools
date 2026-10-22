import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { CodexAccount } from "../../types/codex";
import { isCodexApiKeyAccount } from "../../types/codex";
import type { CodexLocalAccessCreditsPolicy, CodexLocalAccessState } from "../../types/codexLocalAccess";
import { updateCodexLocalAccessCreditsPolicy } from "../../services/codexLocalAccessService";
import { SingleSelectDropdown } from "../SingleSelectDropdown";
import "./CodexCreditsSettings.css";

export function CodexCreditsSettings({ state, accounts, busy, onSaved, maskAccountText }: {
  state: CodexLocalAccessState | null;
  accounts: CodexAccount[];
  busy: boolean;
  onSaved: (state: CodexLocalAccessState) => void;
  maskAccountText: (text: string) => string;
}) {
  const { t } = useTranslation();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const collection = state?.collection;
  const disabled = busy || saving || !collection;
  const save = async (patch: Parameters<typeof updateCodexLocalAccessCreditsPolicy>[0]) => {
    setSaving(true);
    setError("");
    try {
      onSaved(await updateCodexLocalAccessCreditsPolicy(patch));
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };
  const poolIds = new Set([
    ...(collection?.accountIds ?? []),
    ...(collection?.imageGenerationAccountIds ?? []),
    ...(collection?.apiKeys ?? []).flatMap((key) => key.accountIds ?? []),
  ]);
  const members = accounts.filter((account) => poolIds.has(account.id) && !isCodexApiKeyAccount(account));

  return (
    <section className="codex-api-service-panel codex-credits-settings">
      <div className="codex-api-service-panel-head">
        <h2>{t("codex.credits.title", "Credits 使用控制")}</h2>
      </div>
      <div className="codex-api-service-config-list codex-api-service-routing-form">
        <label>
          <span>{t("codex.credits.policy", "全局策略（即时保存）")}</span>
          <SingleSelectDropdown
            value={collection?.creditsPolicy ?? "default"}
            options={[
              { value: "default", label: t("codex.credits.default", "默认情况") },
              { value: "prefer_regular", label: t("codex.credits.prefer", "优先常规额度") },
              { value: "strict_regular", label: t("codex.credits.strict", "严格常规额度") },
            ]}
            onChange={(value) => void save({ policy: value as CodexLocalAccessCreditsPolicy })}
            disabled={disabled}
            ariaLabel={t("codex.credits.title", "Credits 使用控制")}
          />
        </label>
        <p>{t("codex.credits.description", "默认按原调度规则使用账号；优先常规额度需当前 API Key 号池所有账号的常规额度耗尽才允许 credits；严格常规额度禁止转入 credits。账号开关关闭始终优先。")}</p>
        <p>{t("codex.credits.boundary", "依据最近 5 分钟内的额度快照判断；额度未知、过期或重置后会先自动查询额度（最多等待 9 秒，并发合并）。查询失败时安全暂停该账号，具体原因见账号池诊断。无法撤销在途请求，也无法保证上游在单次请求跨越额度边界或其他客户端并发用量时不扣 credits；绝对扣费限制需在上游配置。")}</p>
        {members.map((account) => (
          <label key={account.id} className="codex-credits-account">
            <span>
              {maskAccountText(account.email)}
              <small>{t("codex.credits.allow", "允许使用 credits")}</small>
            </span>
              <input
                type="checkbox"
                checked={collection?.accountCreditsAllowed?.[account.id] !== false}
                disabled={disabled}
                aria-label={`${maskAccountText(account.email)} ${t("codex.credits.allow", "允许使用 credits")}`}
                onChange={(event) => void save({ accountId: account.id, allowed: event.target.checked })}
              />
          </label>
        ))}
        {collection?.creditsPolicy === "strict_regular" && (
          <p>{t("codex.credits.strictHint", "严格模式下所有账号均禁止 credits；账号开关会保留，用于切回其他策略后生效。")}</p>
        )}
        {error && <p role="alert">{error}</p>}
      </div>
    </section>
  );
}
