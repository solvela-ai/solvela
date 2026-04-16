"use client";

import { useState, useEffect } from "react";
import { Save, CheckCircle, Terminal, Info, Key, Trash2, Plus } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { StatusDot } from "@/components/ui/status-dot";
import { setApiKey, clearApiKey, getApiKey } from "@/lib/auth";
import {
  fetchOrgs,
  fetchTeams,
  fetchAuditLogs,
  createTeam,
} from "@/lib/api";
import type { OrgEntry, TeamEntry, AuditLogEntry } from "@/lib/api";

function SectionHeader({ label, description }: { label: string; description?: string }) {
  return (
    <div className="terminal-card-titlebar">
      <span className="terminal-card-dots">
        <span className="terminal-card-dot" />
        <span className="terminal-card-dot" />
        <span className="terminal-card-dot" />
      </span>
      <span className="truncate">{label.toLowerCase().replace(/\s+/g, '.')}</span>
      {description && (
        <span className="ml-2 text-text-tertiary truncate" style={{ fontSize: '10px' }}>{description}</span>
      )}
    </div>
  );
}

function SettingRow({
  label,
  description,
  children,
}: {
  label: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-start justify-between gap-8 py-4 border-b border-border last:border-0">
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-text-primary">{label}</p>
        {description && (
          <p className="mt-0.5 text-xs text-text-tertiary">{description}</p>
        )}
      </div>
      <div className="flex-shrink-0 w-72">{children}</div>
    </div>
  );
}

function Input({
  value,
  onChange,
  placeholder,
  type = "text",
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  type?: string;
}) {
  return (
    <input
      type={type}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      className="w-full rounded border border-border px-3 py-2 text-sm text-text-primary placeholder-text-tertiary bg-bg-inset focus:border-foreground focus:outline-none transition-colors font-mono"
    />
  );
}

function Toggle({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      className={`relative inline-flex h-5 w-9 flex-shrink-0 cursor-pointer rounded-full border border-border transition-colors focus:outline-none ${
        checked ? "bg-foreground" : "bg-bg-surface-raised"
      }`}
    >
      <span
        className={`pointer-events-none inline-block h-[18px] w-[18px] transform rounded-full bg-bg-inset border border-border shadow ring-0 transition-transform ${
          checked ? "translate-x-4" : "translate-x-0"
        }`}
      />
    </button>
  );
}

function EnvVarStatus({
  name,
  set,
  description,
}: {
  name: string;
  set: boolean;
  description: string;
}) {
  return (
    <div className="flex items-center justify-between py-3 border-b border-border last:border-0">
      <div className="min-w-0 flex-1">
        <code className="text-xs font-mono text-text-secondary border border-border rounded px-1.5 py-0.5 bg-bg-inset">
          {name}
        </code>
        <p className="mt-1 text-xs text-text-tertiary">{description}</p>
      </div>
      <StatusDot status={set ? "ok" : "down"} label={set ? "Set" : "Not set"} />
    </div>
  );
}

export default function SettingsPage() {
  const [gatewayUrl, setGatewayUrl] = useState("http://localhost:8402");
  const [dailyBudget, setDailyBudget] = useState("5.00");
  const [monthlyBudget, setMonthlyBudget] = useState("50.00");
  const [promptGuard, setPromptGuard] = useState(true);
  const [rateLimit, setRateLimit] = useState(true);
  const [corsOrigins, setCorsOrigins] = useState("http://localhost:3000");
  const [showEnvSnippet, setShowEnvSnippet] = useState(false);

  const [apiKeyInput, setApiKeyInput] = useState("");
  const [currentApiKey, setCurrentApiKey] = useState<string | null>(null);
  const [apiKeySaved, setApiKeySaved] = useState(false);

  const [orgs, setOrgs] = useState<OrgEntry[]>([]);
  const [selectedOrgId, setSelectedOrgId] = useState<string | null>(null);
  const [teams, setTeams] = useState<TeamEntry[]>([]);
  const [newTeamName, setNewTeamName] = useState("");
  const [creatingTeam, setCreatingTeam] = useState(false);
  const [auditLogs, setAuditLogs] = useState<AuditLogEntry[]>([]);

  useEffect(() => {
    setCurrentApiKey(getApiKey());
  }, []);

  useEffect(() => {
    if (!currentApiKey) return;
    fetchOrgs().then((result) => {
      if (result.ok && result.data.length > 0) {
        setOrgs(result.data);
        setSelectedOrgId(result.data[0].id);
      }
    });
  }, [currentApiKey]);

  useEffect(() => {
    if (!selectedOrgId) return;
    fetchTeams(selectedOrgId).then((result) => {
      if (result.ok) setTeams(result.data);
    });
    fetchAuditLogs(selectedOrgId, { limit: 20 }).then((result) => {
      if (result.ok) setAuditLogs(result.data);
    });
  }, [selectedOrgId]);

  function handleSaveApiKey() {
    if (!apiKeyInput.trim()) return;
    setApiKey(apiKeyInput.trim());
    setCurrentApiKey(apiKeyInput.trim());
    setApiKeyInput("");
    setApiKeySaved(true);
    setTimeout(() => setApiKeySaved(false), 2000);
  }

  function handleClearApiKey() {
    clearApiKey();
    setCurrentApiKey(null);
    setOrgs([]);
    setTeams([]);
    setAuditLogs([]);
    setSelectedOrgId(null);
  }

  async function handleCreateTeam() {
    if (!selectedOrgId || !newTeamName.trim()) return;
    setCreatingTeam(true);
    const result = await createTeam(selectedOrgId, newTeamName.trim());
    if (result.ok) {
      setTeams((prev) => [...prev, result.data]);
      setNewTeamName("");
    }
    setCreatingTeam(false);
  }

  const envSnippet = [
    `NEXT_PUBLIC_GATEWAY_URL=${gatewayUrl}`,
    `RCR_CORS_ORIGINS=${corsOrigins}`,
    `RCR_DAILY_BUDGET_USDC=${dailyBudget}`,
    `RCR_MONTHLY_BUDGET_USDC=${monthlyBudget}`,
    `RCR_PROMPT_GUARD_ENABLED=${promptGuard}`,
    `RCR_RATE_LIMIT_ENABLED=${rateLimit}`,
  ].join("\n");

  return (
    <div className="flex flex-col h-full">
      <Topbar title="Settings" subtitle="Gateway configuration and budget limits" />

      <div className="flex-1 p-6 space-y-5 max-w-3xl">
        {/* API Key */}
        <div className="terminal-card overflow-hidden">
          <SectionHeader
            label="API Key"
            description="Authenticate with org-scoped endpoints. Stored in localStorage only."
          />
          <div className="terminal-card-screen space-y-3">
            <div className="flex items-center gap-1.5 text-xs text-text-tertiary font-mono mb-1">
              <Key size={11} />
              <span>rcr_k_... key grants access to /orgs and audit endpoints</span>
            </div>

            {currentApiKey ? (
              <div className="flex items-center justify-between rounded border border-border px-3 py-2">
                <div className="flex items-center gap-2">
                  <CheckCircle size={12} className="text-success" />
                  <code className="text-xs font-mono text-text-secondary">
                    {currentApiKey.slice(0, 10)}...
                  </code>
                  <span className="text-xs text-text-tertiary">configured</span>
                </div>
                <button
                  onClick={handleClearApiKey}
                  className="flex items-center gap-1 text-xs text-error hover:text-error transition-colors"
                >
                  <Trash2 size={11} />
                  Clear
                </button>
              </div>
            ) : (
              <p className="text-xs text-text-tertiary font-mono">No API key configured</p>
            )}

            <div className="flex gap-2">
              <input
                type="password"
                value={apiKeyInput}
                onChange={(e) => setApiKeyInput(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && handleSaveApiKey()}
                placeholder="rcr_k_..."
                className="flex-1 rounded border border-border px-3 py-2 text-sm text-text-primary placeholder-text-tertiary bg-bg-inset focus:border-foreground focus:outline-none transition-colors font-mono"
              />
              <button
                onClick={handleSaveApiKey}
                disabled={!apiKeyInput.trim()}
                className="flex items-center gap-1.5 rounded border border-border px-4 py-2 text-sm font-medium text-text-primary hover:bg-bg-surface disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
              >
                {apiKeySaved ? <CheckCircle size={12} /> : <Save size={12} />}
                {apiKeySaved ? "Saved" : "Save"}
              </button>
            </div>
          </div>
        </div>

        {/* Team Management */}
        <div className="terminal-card overflow-hidden">
          <SectionHeader
            label="Team Management"
            description="Manage teams within your organization. Requires a valid API key."
          />
          <div className="terminal-card-screen">
            {!currentApiKey ? (
              <p className="text-xs text-text-tertiary font-mono">Configure an API key to view team management.</p>
            ) : orgs.length === 0 ? (
              <p className="text-xs text-text-tertiary font-mono">No organizations found.</p>
            ) : (
              <div className="space-y-4">
                {orgs.length > 1 && (
                  <div className="flex items-center gap-2">
                    <label className="text-xs text-text-tertiary font-mono">Org:</label>
                    <select
                      value={selectedOrgId ?? ""}
                      onChange={(e) => setSelectedOrgId(e.target.value)}
                      className="rounded border border-border px-2 py-1 text-xs text-text-primary bg-bg-inset focus:outline-none focus:border-foreground font-mono"
                    >
                      {orgs.map((o) => (
                        <option key={o.id} value={o.id}>{o.name}</option>
                      ))}
                    </select>
                  </div>
                )}

                <div className="rounded border border-border divide-y divide-border">
                  {teams.length === 0 ? (
                    <p className="px-4 py-3 text-xs text-text-tertiary font-mono">No teams yet.</p>
                  ) : (
                    teams.map((team) => (
                      <div key={team.id} className="flex items-center justify-between px-4 py-3">
                        <div>
                          <p className="text-sm font-medium text-text-primary">{team.name}</p>
                          {team.wallet_count !== undefined && (
                            <p className="text-xs text-text-tertiary font-mono">
                              {team.wallet_count} wallet{team.wallet_count !== 1 ? "s" : ""}
                            </p>
                          )}
                        </div>
                        {team.budget && (
                          <div className="text-xs text-text-tertiary font-mono text-right">
                            {team.budget.daily_limit != null && (
                              <p>Daily: {team.budget.daily_limit} USDC</p>
                            )}
                            {team.budget.monthly_limit != null && (
                              <p>Monthly: {team.budget.monthly_limit} USDC</p>
                            )}
                          </div>
                        )}
                      </div>
                    ))
                  )}
                </div>

                <div className="flex gap-2">
                  <input
                    type="text"
                    value={newTeamName}
                    onChange={(e) => setNewTeamName(e.target.value)}
                    onKeyDown={(e) => e.key === "Enter" && handleCreateTeam()}
                    placeholder="New team name"
                    className="flex-1 rounded border border-border px-3 py-2 text-sm text-text-primary placeholder-text-tertiary bg-bg-inset focus:border-foreground focus:outline-none transition-colors font-mono"
                  />
                  <button
                    onClick={handleCreateTeam}
                    disabled={!newTeamName.trim() || creatingTeam}
                    className="flex items-center gap-1.5 rounded border border-border px-4 py-2 text-sm font-medium text-text-primary hover:bg-bg-surface disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                  >
                    <Plus size={12} />
                    {creatingTeam ? "Creating..." : "Create"}
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>

        {/* Audit Log */}
        <div className="terminal-card overflow-hidden">
          <SectionHeader
            label="Audit Log"
            description="Recent organization activity (last 20 entries)."
          />
          <div className="terminal-card-screen">
            {!currentApiKey ? (
              <p className="text-xs text-text-tertiary font-mono">Configure an API key to view audit logs.</p>
            ) : auditLogs.length === 0 ? (
              <p className="text-xs text-text-tertiary font-mono">No audit log entries found.</p>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full text-xs">
                  <thead>
                    <tr className="border-b border-border">
                      <th className="pb-2 text-left font-medium text-text-tertiary font-mono uppercase tracking-wide">Action</th>
                      <th className="pb-2 text-left font-medium text-text-tertiary font-mono uppercase tracking-wide">Resource</th>
                      <th className="pb-2 text-left font-medium text-text-tertiary font-mono uppercase tracking-wide">Time</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-border">
                    {auditLogs.map((entry) => (
                      <tr key={entry.id}>
                        <td className="py-2 pr-4 font-mono text-text-primary">{entry.action}</td>
                        <td className="py-2 pr-4 text-text-secondary font-mono">{entry.resource_type}</td>
                        <td className="py-2 text-text-tertiary font-mono">
                          {new Date(entry.created_at).toLocaleString()}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </div>

        {/* Gateway */}
        <div className="terminal-card overflow-hidden">
          <SectionHeader
            label="Gateway"
            description="Solvela API endpoint configuration"
          />
          <div className="terminal-card-screen" style={{ paddingTop: '0', paddingBottom: '0' }}>
            <SettingRow
              label="Gateway URL"
              description="Base URL of your Solvela gateway (NEXT_PUBLIC_GATEWAY_URL)"
            >
              <Input value={gatewayUrl} onChange={setGatewayUrl} placeholder="http://localhost:8402" />
            </SettingRow>
            <SettingRow
              label="CORS Origins"
              description="Comma-separated allowed origins (RCR_CORS_ORIGINS)"
            >
              <Input value={corsOrigins} onChange={setCorsOrigins} placeholder="http://localhost:3000" />
            </SettingRow>
          </div>
        </div>

        {/* Budget */}
        <div className="terminal-card overflow-hidden">
          <SectionHeader
            label="Budget Limits"
            description="Per-wallet spend limits in USDC. Requests exceeding limits return 402."
          />
          <div className="terminal-card-screen" style={{ paddingTop: '0', paddingBottom: '0' }}>
            <SettingRow
              label="Daily Limit (USDC)"
              description="Maximum spend per wallet per day"
            >
              <Input value={dailyBudget} onChange={setDailyBudget} placeholder="5.00" type="number" />
            </SettingRow>
            <SettingRow
              label="Monthly Limit (USDC)"
              description="Maximum spend per wallet per month"
            >
              <Input value={monthlyBudget} onChange={setMonthlyBudget} placeholder="50.00" type="number" />
            </SettingRow>
          </div>
        </div>

        {/* Security */}
        <div className="terminal-card overflow-hidden">
          <SectionHeader
            label="Security"
            description="Middleware and protection settings"
          />
          <div className="terminal-card-screen" style={{ paddingTop: '0', paddingBottom: '0' }}>
            <SettingRow
              label="Prompt Guard"
              description="Block injection attacks, jailbreaks, and PII in prompts"
            >
              <Toggle checked={promptGuard} onChange={setPromptGuard} />
            </SettingRow>
            <SettingRow
              label="Rate Limiting"
              description="Per-wallet token-bucket rate limiter"
            >
              <Toggle checked={rateLimit} onChange={setRateLimit} />
            </SettingRow>
          </div>
        </div>

        {/* Wallet & Keys */}
        <div className="terminal-card overflow-hidden">
          <SectionHeader
            label="Wallet &amp; Keys"
          />
          <div className="terminal-card-screen space-y-4">
            <div className="flex items-start gap-2 rounded border border-border p-3">
              <Info size={13} className="text-text-tertiary mt-0.5 flex-shrink-0" />
              <p className="text-xs text-text-secondary font-mono">
                Private keys are never entered here. Set{" "}
                <code>SOLANA_WALLET_KEY</code> in your{" "}
                <code>.env</code> file or shell environment.
              </p>
            </div>
            <div>
              <EnvVarStatus
                name="SOLANA_WALLET_KEY"
                set={false}
                description="Base58 Solana keypair for signing x402 payments — set in .env, never in this UI"
              />
              <EnvVarStatus
                name="RCR_SOLANA_RPC_URL"
                set={true}
                description="Solana RPC endpoint used by the gateway"
              />
              <EnvVarStatus
                name="RCR_SOLANA_RECIPIENT_WALLET"
                set={true}
                description="Gateway payment destination wallet"
              />
            </div>
          </div>
        </div>

        {/* Generate .env snippet */}
        <div className="space-y-3">
          <button
            onClick={() => setShowEnvSnippet(true)}
            className="flex items-center gap-2 rounded border border-border px-5 py-2.5 text-sm font-medium text-text-primary hover:bg-bg-surface transition-colors"
          >
            <Save size={13} />
            Generate .env Snippet
          </button>

          {showEnvSnippet && (
            <div className="terminal-card overflow-hidden">
              <div className="terminal-card-titlebar">
                <span className="terminal-card-dots">
                  <span className="terminal-card-dot" />
                  <span className="terminal-card-dot" />
                  <span className="terminal-card-dot terminal-card-dot--accent" />
                </span>
                <Terminal size={11} className="text-text-tertiary" />
                <span>config.env</span>
                <CheckCircle size={11} className="text-success ml-auto" />
              </div>
              <div className="terminal-card-screen" style={{ padding: '1rem' }}>
                <pre className="text-xs font-mono text-text-primary whitespace-pre-wrap break-all">
                  {envSnippet}
                </pre>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
