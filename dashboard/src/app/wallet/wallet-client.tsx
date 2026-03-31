"use client";

import { useState } from "react";
import { Copy, Check, ExternalLink, ArrowUpRight } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { StatusDot } from "@/components/ui/status-dot";
import { WALLET_TXS, DASHBOARD_STATS } from "@/lib/mock-data";
import { formatUSDC, formatNumber } from "@/lib/utils";
import type { EscrowConfig } from "@/types";

interface WalletPageClientProps {
  recipientWallet: string;
  totalSpend?: number;
  topWallets?: Array<{ wallet: string; requests: number; cost: number }>;
  escrowConfig?: EscrowConfig | null;
  usingMockData?: boolean;
}

export function WalletPageClient({
  recipientWallet,
  totalSpend,
  topWallets,
  escrowConfig,
  usingMockData = false,
}: WalletPageClientProps) {
  const [copied, setCopied] = useState(false);
  const balance = totalSpend ?? DASHBOARD_STATS.walletBalance;
  const addr = recipientWallet;
  const isConfigured = addr.length > 20 && !addr.startsWith("Configure");
  const short = isConfigured
    ? `${addr.slice(0, 8)}...${addr.slice(-8)}`
    : addr;

  return (
    <div className="flex flex-col h-full">
      <Topbar title="Wallet" subtitle="Solana USDC-SPL balance and transaction history" />

      <div className="flex-1 p-6 space-y-6">
        {/* Balance card */}
        <div className="rounded-xl border border-gray-200 bg-white p-6 shadow-sm">
          <div className="flex items-start justify-between">
            <div>
              <p className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                {topWallets ? "Total Platform Spend" : "USDC Balance"}
              </p>
              <p className="mt-1 text-4xl font-bold text-gray-900 tabular-nums">
                {formatUSDC(balance, 2)}
              </p>
              <p className="mt-0.5 text-sm text-gray-500">
                {topWallets ? "USDC on Solana mainnet (last 30 days)" : `≈ ${balance.toFixed(2)} USDC on Solana mainnet`}
              </p>
            </div>
            <StatusDot status="ok" label="Connected" />
          </div>

          <div className="mt-4 flex items-center gap-2">
            <code className="rounded-lg bg-gray-50 border border-gray-200 px-3 py-1.5 text-xs font-mono text-gray-700">
              {short}
            </code>
            <button
              className={`rounded-lg border border-gray-200 p-1.5 hover:bg-gray-50 transition-colors ${copied ? "text-green-600" : "text-gray-500"}`}
              aria-label="Copy address"
              onClick={() => {
                if (isConfigured) {
                  navigator.clipboard.writeText(addr).then(
                    () => { setCopied(true); setTimeout(() => setCopied(false), 1500); },
                    (err) => { console.warn("[WalletPage] Clipboard write failed:", err); }
                  );
                }
              }}
            >
              {copied ? <Check size={13} /> : <Copy size={13} />}
            </button>
            <a
              href={`https://solscan.io/account/${addr}`}
              target="_blank"
              rel="noopener noreferrer"
              className="rounded-lg border border-gray-200 p-1.5 text-gray-500 hover:bg-gray-50 transition-colors"
              aria-label="View on Solscan"
            >
              <ExternalLink size={13} />
            </a>
          </div>
        </div>

        {/* Escrow config (if available) */}
        {escrowConfig && (
          <div className="rounded-xl border border-gray-200 bg-white p-5 shadow-sm">
            <h2 className="text-sm font-semibold text-gray-900 mb-3">
              Escrow Configuration
            </h2>
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 text-sm">
              <div>
                <p className="text-xs text-gray-500">Network</p>
                <p className="font-medium text-gray-900">{escrowConfig.network}</p>
              </div>
              <div>
                <p className="text-xs text-gray-500">Current Slot</p>
                <p className="font-medium text-gray-900 tabular-nums">{formatNumber(escrowConfig.current_slot)}</p>
              </div>
              <div>
                <p className="text-xs text-gray-500">USDC Mint</p>
                <code className="text-xs font-mono text-gray-700">{escrowConfig.usdc_mint}</code>
              </div>
              <div>
                <p className="text-xs text-gray-500">Program ID</p>
                <code className="text-xs font-mono text-gray-700">{escrowConfig.escrow_program_id}</code>
              </div>
            </div>
          </div>
        )}

        {/* Top wallets table (from API) or recent transactions (mock fallback) */}
        {topWallets ? (
          <div className="rounded-xl border border-gray-200 bg-white shadow-sm overflow-hidden">
            <div className="px-5 py-4 border-b border-gray-100">
              <h2 className="text-sm font-semibold text-gray-900">
                Top Wallets
              </h2>
            </div>
            <div className="divide-y divide-gray-100">
              {topWallets.map((w) => (
                <div
                  key={w.wallet}
                  className="flex items-center gap-4 px-5 py-3 hover:bg-gray-50 transition-colors"
                >
                  <code className="text-xs font-mono text-gray-600 min-w-0 truncate">
                    {w.wallet}
                  </code>
                  <div className="ml-auto flex items-center gap-6">
                    <span className="text-sm font-medium text-gray-900 tabular-nums">
                      {formatUSDC(w.cost, 4)}
                    </span>
                    <span className="text-xs text-gray-400 tabular-nums">
                      {formatNumber(w.requests)} req
                    </span>
                  </div>
                </div>
              ))}
            </div>
          </div>
        ) : (
          /* Fallback: mock transaction history */
          <div className="rounded-xl border border-gray-200 bg-white shadow-sm overflow-hidden">
            <div className="px-5 py-4 border-b border-gray-100 flex items-center justify-between">
              <h2 className="text-sm font-semibold text-gray-900">
                Recent Transactions
              </h2>
              <a
                href={`https://solscan.io/account/${addr}`}
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-1 text-xs text-blue-600 hover:underline"
              >
                View all <ExternalLink size={10} />
              </a>
            </div>
            <div className="divide-y divide-gray-100">
              {WALLET_TXS.map((tx) => (
                <div
                  key={tx.signature}
                  className="flex items-center gap-4 px-5 py-3 hover:bg-gray-50 transition-colors"
                >
                  <div className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-full bg-orange-50 text-orange-600">
                    <ArrowUpRight size={14} />
                  </div>
                  <div className="flex-1 min-w-0">
                    <p className="text-sm font-medium text-gray-900 truncate">
                      {tx.model}
                    </p>
                    <p className="text-xs text-gray-400">
                      {tx.timestamp} ·{" "}
                      <a
                        href={`https://solscan.io/tx/${tx.signature}`}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="hover:underline text-blue-500"
                      >
                        {tx.signature}
                      </a>
                    </p>
                  </div>
                  <div className="text-right">
                    <p className="text-sm font-medium text-gray-900 tabular-nums">
                      −${tx.amount}
                    </p>
                    <p className="text-xs text-gray-400">USDC</p>
                  </div>
                  <StatusDot
                    status={
                      tx.status === "confirmed"
                        ? "ok"
                        : tx.status === "pending"
                        ? "degraded"
                        : "down"
                    }
                    label={tx.status}
                  />
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Funding instructions */}
        <div className="rounded-xl border border-blue-100 bg-blue-50 p-5">
          <h2 className="text-sm font-semibold text-blue-900 mb-2">
            Fund Your Wallet
          </h2>
          <ol className="space-y-1.5 text-sm text-blue-800 list-decimal list-inside">
            <li>
              Send USDC-SPL to your wallet address above on Solana mainnet
            </li>
            <li>
              USDC Mint:{" "}
              <code className="font-mono text-xs bg-blue-100 rounded px-1 py-0.5">
                EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v
              </code>
            </li>
            <li>
              Your wallet must hold a small amount of SOL for transaction fees
              (~0.002 SOL)
            </li>
            <li>
              Payments are deducted automatically per API call via x402 protocol
            </li>
          </ol>
        </div>
      </div>
    </div>
  );
}
