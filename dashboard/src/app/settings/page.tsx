"use client";

import { useState } from "react";
import { Save, CheckCircle, Terminal, Info } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { StatusDot } from "@/components/ui/status-dot";

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
        {/* Gateway */}
        <div className="rounded-xl border border-gray-200 bg-white p-6 shadow-sm">
          <h2 className="text-sm font-semibold text-gray-900 mb-1">Gateway</h2>
          <p className="text-xs text-gray-500 mb-4">
            RustyClawRouter API endpoint configuration
          </p>
          <div>
            <SettingRow
              label="Gateway URL"
              description="Base URL of your RustyClawRouter gateway (NEXT_PUBLIC_GATEWAY_URL)"
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
