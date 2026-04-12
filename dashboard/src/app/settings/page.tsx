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
    <div className="flex items-start justify-between gap-8 py-5 border-b border-gray-100 last:border-0">
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-gray-900">{label}</p>
        {description && (
          <p className="mt-0.5 text-xs text-gray-500">{description}</p>
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
      className="w-full rounded-lg border border-gray-200 px-3 py-2 text-sm text-gray-900 placeholder-gray-400 focus:border-orange-400 focus:outline-none focus:ring-2 focus:ring-orange-100 transition-colors"
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
      className={`relative inline-flex h-5 w-9 flex-shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors focus:outline-none ${
        checked ? "bg-orange-500" : "bg-gray-200"
      }`}
    >
      <span
        className={`pointer-events-none inline-block h-4 w-4 transform rounded-full bg-white shadow ring-0 transition-transform ${
          checked ? "translate-x-4" : "translate-x-0"
        }`}
      />
    </button>
  );
}

/** Read-only env-var status row: shows whether a variable is configured. */
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
    <div className="flex items-center justify-between py-4 border-b border-gray-100 last:border-0">
      <div className="min-w-0 flex-1">
        <code className="text-xs font-mono font-medium text-gray-700 bg-gray-50 rounded px-1.5 py-0.5 border border-gray-200">
          {name}
        </code>
        <p className="mt-1 text-xs text-gray-500">{description}</p>
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

  // Pending state: true settings persistence requires a gateway API.
  // For now, show the generated .env snippet the user should apply manually.
  const [showEnvSnippet, setShowEnvSnippet] = useState(false);

  // API Key section
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [currentApiKey, setCurrentApiKey] = useState<string | null>(null);
  const [apiKeySaved, setApiKeySaved] = useState(false);

  // Team management
  const [orgs, setOrgs] = useState<OrgEntry[]>([]);
  const [selectedOrgId, setSelectedOrgId] = useState<string | null>(null);
  const [teams, setTeams] = useState<TeamEntry[]>([]);
  const [newTeamName, setNewTeamName] = useState("");
  const [creatingTeam, setCreatingTeam] = useState(false);

  // Audit log
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

      <div className="flex-1 p-6 space-y-6 max-w-3xl">
        {/* API Key */}
        <div className="rounded-xl border border-gray-200 bg-white p-6 shadow-sm">
          <div className="flex items-center gap-2 mb-1">
            <Key size={14} className="text-gray-500" />
            <h2 className="text-sm font-semibold text-gray-900">API Key</h2>
          </div>
          <p className="text-xs text-gray-500 mb-4">
            Paste your <code className="font-mono">rcr_k_...</code> API key to
            authenticate with org-scoped endpoints. Stored in localStorage only.
          </p>

          <div className="space-y-3">
            {currentApiKey ? (
              <div className="flex items-center justify-between rounded-lg border border-green-200 bg-green-50 px-3 py-2">
                <div className="flex items-center gap-2">
                  <CheckCircle size={13} className="text-green-600" />
                  <code className="text-xs font-mono text-green-800">
                    {currentApiKey.slice(0, 10)}...
                  </code>
                  <span className="text-xs text-green-700">configured</span>
                </div>
                <button
                  onClick={handleClearApiKey}
                  className="flex items-center gap-1 text-xs text-red-600 hover:text-red-700 transition-colors"
                >
                  <Trash2 size={12} />
                  Clear
                </button>
              </div>
            ) : (
              <p className="text-xs text-gray-400 italic">No API key configured</p>
            )}

            <div className="flex gap-2">
              <input
                type="password"
                value={apiKeyInput}
                onChange={(e) => setApiKeyInput(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && handleSaveApiKey()}
                placeholder="rcr_k_..."
                className="flex-1 rounded-lg border border-gray-200 px-3 py-2 text-sm text-gray-900 placeholder-gray-400 focus:border-orange-400 focus:outline-none focus:ring-2 focus:ring-orange-100 transition-colors"
              />
              <button
                onClick={handleSaveApiKey}
                disabled={!apiKeyInput.trim()}
                className="flex items-center gap-1.5 rounded-lg px-4 py-2 text-sm font-medium text-white bg-orange-500 hover:bg-orange-600 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
              >
                {apiKeySaved ? <CheckCircle size={13} /> : <Save size={13} />}
                {apiKeySaved ? "Saved" : "Save"}
              </button>
            </div>
          </div>
        </div>

        {/* Team Management */}
        <div className="rounded-xl border border-gray-200 bg-white p-6 shadow-sm">
          <h2 className="text-sm font-semibold text-gray-900 mb-1">Team Management</h2>
          <p className="text-xs text-gray-500 mb-4">
            Manage teams within your organization. Requires a valid API key above.
          </p>

          {!currentApiKey ? (
            <p className="text-xs text-gray-400 italic">
              Configure an API key to view team management.
            </p>
          ) : orgs.length === 0 ? (
            <p className="text-xs text-gray-400 italic">No organizations found.</p>
          ) : (
            <div className="space-y-4">
              {orgs.length > 1 && (
                <div className="flex items-center gap-2">
                  <label className="text-xs text-gray-600 font-medium">Org:</label>
                  <select
                    value={selectedOrgId ?? ""}
                    onChange={(e) => setSelectedOrgId(e.target.value)}
                    className="rounded border border-gray-200 px-2 py-1 text-xs text-gray-900 focus:outline-none focus:border-orange-400"
                  >
                    {orgs.map((o) => (
                      <option key={o.id} value={o.id}>
                        {o.name}
                      </option>
                    ))}
                  </select>
                </div>
              )}

              {/* Teams list */}
              <div className="divide-y divide-gray-100 rounded-lg border border-gray-200">
                {teams.length === 0 ? (
                  <p className="px-4 py-3 text-xs text-gray-400 italic">No teams yet.</p>
                ) : (
                  teams.map((team) => (
                    <div key={team.id} className="flex items-center justify-between px-4 py-3">
                      <div>
                        <p className="text-sm font-medium text-gray-900">{team.name}</p>
                        {team.wallet_count !== undefined && (
                          <p className="text-xs text-gray-500">
                            {team.wallet_count} wallet{team.wallet_count !== 1 ? "s" : ""}
                          </p>
                        )}
                      </div>
                      {team.budget && (
                        <div className="text-xs text-gray-500 text-right">
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

              {/* Create team */}
              <div className="flex gap-2">
                <input
                  type="text"
                  value={newTeamName}
                  onChange={(e) => setNewTeamName(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && handleCreateTeam()}
                  placeholder="New team name"
                  className="flex-1 rounded-lg border border-gray-200 px-3 py-2 text-sm text-gray-900 placeholder-gray-400 focus:border-orange-400 focus:outline-none focus:ring-2 focus:ring-orange-100 transition-colors"
                />
                <button
                  onClick={handleCreateTeam}
                  disabled={!newTeamName.trim() || creatingTeam}
                  className="flex items-center gap-1.5 rounded-lg px-4 py-2 text-sm font-medium text-white bg-orange-500 hover:bg-orange-600 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                >
                  <Plus size={13} />
                  {creatingTeam ? "Creating..." : "Create Team"}
                </button>
              </div>
            </div>
          )}
        </div>

        {/* Audit Log */}
        <div className="rounded-xl border border-gray-200 bg-white p-6 shadow-sm">
          <h2 className="text-sm font-semibold text-gray-900 mb-1">Audit Log</h2>
          <p className="text-xs text-gray-500 mb-4">Recent organization activity (last 20 entries).</p>

          {!currentApiKey ? (
            <p className="text-xs text-gray-400 italic">
              Configure an API key to view audit logs.
            </p>
          ) : auditLogs.length === 0 ? (
            <p className="text-xs text-gray-400 italic">No audit log entries found.</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-xs">
                <thead>
                  <tr className="border-b border-gray-100">
                    <th className="pb-2 text-left font-medium text-gray-600">Action</th>
                    <th className="pb-2 text-left font-medium text-gray-600">Resource</th>
                    <th className="pb-2 text-left font-medium text-gray-600">Time</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-gray-50">
                  {auditLogs.map((entry) => (
                    <tr key={entry.id}>
                      <td className="py-2 pr-4 font-mono text-gray-800">{entry.action}</td>
                      <td className="py-2 pr-4 text-gray-600">{entry.resource_type}</td>
                      <td className="py-2 text-gray-500">
                        {new Date(entry.created_at).toLocaleString()}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>

        {/* Gateway */}
        <div className="rounded-xl border border-gray-200 bg-white p-6 shadow-sm">
          <h2 className="text-sm font-semibold text-gray-900 mb-1">Gateway</h2>
          <p className="text-xs text-gray-500 mb-4">
            Solvela API endpoint configuration
          </p>
          <div>
            <SettingRow
              label="Gateway URL"
              description="Base URL of your Solvela gateway (NEXT_PUBLIC_GATEWAY_URL)"
            >
              <Input
                value={gatewayUrl}
                onChange={setGatewayUrl}
                placeholder="http://localhost:8402"
              />
            </SettingRow>
            <SettingRow
              label="CORS Origins"
              description="Comma-separated allowed origins (RCR_CORS_ORIGINS)"
            >
              <Input
                value={corsOrigins}
                onChange={setCorsOrigins}
                placeholder="http://localhost:3000"
              />
            </SettingRow>
          </div>
        </div>

        {/* Budget */}
        <div className="rounded-xl border border-gray-200 bg-white p-6 shadow-sm">
          <h2 className="text-sm font-semibold text-gray-900 mb-1">
            Budget Limits
          </h2>
          <p className="text-xs text-gray-500 mb-4">
            Per-wallet spend limits in USDC. Requests exceeding limits return 402.
          </p>
          <div>
            <SettingRow
              label="Daily Limit (USDC)"
              description="Maximum spend per wallet per day"
            >
              <Input
                value={dailyBudget}
                onChange={setDailyBudget}
                placeholder="5.00"
                type="number"
              />
            </SettingRow>
            <SettingRow
              label="Monthly Limit (USDC)"
              description="Maximum spend per wallet per month"
            >
              <Input
                value={monthlyBudget}
                onChange={setMonthlyBudget}
                placeholder="50.00"
                type="number"
              />
            </SettingRow>
          </div>
        </div>

        {/* Security */}
        <div className="rounded-xl border border-gray-200 bg-white p-6 shadow-sm">
          <h2 className="text-sm font-semibold text-gray-900 mb-1">Security</h2>
          <p className="text-xs text-gray-500 mb-4">
            Middleware and protection settings
          </p>
          <div>
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

        {/* Wallet / keys — read-only env-var status panel */}
        <div className="rounded-xl border border-gray-200 bg-white p-6 shadow-sm">
          <h2 className="text-sm font-semibold text-gray-900 mb-1">
            Wallet &amp; Keys
          </h2>
          <div className="flex items-start gap-2 mb-4 rounded-lg border border-blue-100 bg-blue-50 p-3">
            <Info size={14} className="text-blue-600 mt-0.5 flex-shrink-0" />
            <p className="text-xs text-blue-800">
              Private keys are never entered here. Set{" "}
              <code className="font-mono">SOLANA_WALLET_KEY</code> in your{" "}
              <code className="font-mono">.env</code> file or shell environment.
              The dashboard shows connection status only.
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

        {/* Apply button — generates env snippet */}
        <div className="space-y-3">
          <button
            onClick={() => setShowEnvSnippet(true)}
            className="flex items-center gap-2 rounded-lg px-5 py-2.5 text-sm font-medium text-white bg-orange-500 hover:bg-orange-600 transition-colors"
          >
            <Save size={14} />
            Generate .env Snippet
          </button>

          {showEnvSnippet && (
            <div className="rounded-xl border border-gray-200 bg-gray-50 p-4">
              <div className="flex items-center gap-2 mb-3">
                <Terminal size={13} className="text-gray-500" />
                <p className="text-xs font-medium text-gray-700">
                  Add to your gateway <code className="font-mono">.env</code> file:
                </p>
                <CheckCircle size={13} className="text-green-500 ml-auto" />
              </div>
              <pre className="text-xs font-mono text-gray-800 whitespace-pre-wrap break-all">
                {envSnippet}
              </pre>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
