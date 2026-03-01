"use client";

import { useState } from "react";
import { Save, Eye, EyeOff } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";

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

export default function SettingsPage() {
  const [gatewayUrl, setGatewayUrl] = useState("http://localhost:8402");
  const [dailyBudget, setDailyBudget] = useState("5.00");
  const [monthlyBudget, setMonthlyBudget] = useState("50.00");
  const [promptGuard, setPromptGuard] = useState(true);
  const [rateLimit, setRateLimit] = useState(true);
  const [corsOrigins, setCorsOrigins] = useState("http://localhost:3000");
  const [walletKey, setWalletKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [saved, setSaved] = useState(false);

  function handleSave() {
    // In production, POST to gateway /settings or write to .env
    setSaved(true);
    setTimeout(() => setSaved(false), 2000);
  }

  return (
    <div className="flex flex-col h-full">
      <Topbar title="Settings" subtitle="Gateway configuration and budget limits" />

      <div className="flex-1 p-6 space-y-6 max-w-3xl">
        {/* Gateway */}
        <div className="rounded-xl border border-gray-200 bg-white p-6 shadow-sm">
          <h2 className="text-sm font-semibold text-gray-900 mb-1">
            Gateway
          </h2>
          <p className="text-xs text-gray-500 mb-4">
            RustyClawRouter API endpoint configuration
          </p>
          <div>
            <SettingRow
              label="Gateway URL"
              description="Base URL of your RustyClawRouter gateway"
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
          <h2 className="text-sm font-semibold text-gray-900 mb-1">
            Security
          </h2>
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

        {/* Wallet */}
        <div className="rounded-xl border border-gray-200 bg-white p-6 shadow-sm">
          <h2 className="text-sm font-semibold text-gray-900 mb-1">
            Wallet
          </h2>
          <p className="text-xs text-gray-500 mb-4">
            Solana keypair for signing x402 payments (stored in SOLANA_WALLET_KEY)
          </p>
          <SettingRow
            label="Private Key"
            description="Base58-encoded 64-byte Solana keypair secret key"
          >
            <div className="relative">
              <input
                type={showKey ? "text" : "password"}
                value={walletKey}
                onChange={(e) => setWalletKey(e.target.value)}
                placeholder="Base58 keypair…"
                className="w-full rounded-lg border border-gray-200 px-3 py-2 pr-9 text-sm text-gray-900 placeholder-gray-400 focus:border-orange-400 focus:outline-none focus:ring-2 focus:ring-orange-100 transition-colors font-mono"
              />
              <button
                type="button"
                onClick={() => setShowKey(!showKey)}
                className="absolute right-2.5 top-2.5 text-gray-400 hover:text-gray-600"
              >
                {showKey ? <EyeOff size={14} /> : <Eye size={14} />}
              </button>
            </div>
          </SettingRow>
        </div>

        {/* Save button */}
        <button
          onClick={handleSave}
          className={`flex items-center gap-2 rounded-lg px-5 py-2.5 text-sm font-medium text-white transition-colors ${
            saved
              ? "bg-green-500"
              : "bg-orange-500 hover:bg-orange-600"
          }`}
        >
          <Save size={14} />
          {saved ? "Saved!" : "Save Settings"}
        </button>
      </div>
    </div>
  );
}
