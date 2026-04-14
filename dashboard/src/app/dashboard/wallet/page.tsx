import { Copy, ExternalLink, ArrowUpRight, AlertTriangle } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { StatusDot } from "@/components/ui/status-dot";
import { WALLET_TXS, DASHBOARD_STATS } from "@/lib/mock-data";
import { fetchAdminStats, fetchEscrowConfig } from "@/lib/api";
import { formatUSDC, formatNumber } from "@/lib/utils";

// Read from server-side env var — never a client-side public var (no private key here,
// but the recipient wallet address is also fine as a non-secret display field).
const RECIPIENT_WALLET =
  process.env.RCR_SOLANA_RECIPIENT_WALLET ??
  "Configure RCR_SOLANA_RECIPIENT_WALLET in .env";

export default async function WalletPage() {
  const [statsResponse, escrowConfig] = await Promise.all([
    fetchAdminStats(30),
    fetchEscrowConfig(),
  ]);

  const usingMockData = !statsResponse;

  const totalSpend = statsResponse
    ? parseFloat(statsResponse.summary.total_cost_usdc)
    : DASHBOARD_STATS.totalSpend;

  const topWallets = statsResponse
    ? statsResponse.top_wallets.map((w) => ({
        wallet: w.wallet,
        requests: w.requests,
        cost: parseFloat(w.cost_usdc),
      }))
    : null;

  const addr = RECIPIENT_WALLET;
  const isConfigured = addr.length > 20 && !addr.startsWith("Configure");
  const short = isConfigured
    ? `${addr.slice(0, 8)}...${addr.slice(-8)}`
    : addr;

  return (
    <div className="flex flex-col h-full">
      <Topbar title="Wallet" subtitle="Solana USDC-SPL balance and transaction history" />

      <div className="flex-1 p-6 space-y-6">
        {/* Mock data warning banner */}
        {usingMockData && (
          <div className="flex items-center gap-2 rounded-lg border border-warning/20 bg-warning/10 px-4 py-2.5 text-sm text-warning">
            <AlertTriangle size={14} className="flex-shrink-0" />
            <span>
              Unable to reach gateway API. Showing sample data.
            </span>
          </div>
        )}

        {/* Balance card */}
        <div className="rounded-xl border border-border bg-bg-surface p-6">
          <div className="flex items-start justify-between">
            <div>
              <p className="text-xs font-medium text-text-secondary uppercase tracking-wide">
                Total Platform Spend
              </p>
              <p className="mt-1 text-4xl font-bold text-text-primary tabular-nums">
                {formatUSDC(totalSpend, 2)}
              </p>
              <p className="mt-0.5 text-sm text-text-secondary">USDC on Solana mainnet (last 30 days)</p>
            </div>
            <StatusDot status="ok" label="Connected" />
          </div>

          <div className="mt-4 flex items-center gap-2">
            <code className="rounded-lg bg-bg-inset border border-border px-3 py-1.5 text-xs font-mono text-text-secondary">
              {short}
            </code>
            <button
              className="rounded-lg border border-border p-1.5 text-text-secondary hover:bg-bg-surface-hover transition-colors"
              title="Copy address"
              aria-label="Copy address"
            >
              <Copy size={13} />
            </button>
            <a
              href={`https://solscan.io/account/${addr}`}
              target="_blank"
              rel="noopener noreferrer"
              className="rounded-lg border border-border p-1.5 text-text-secondary hover:bg-bg-surface-hover transition-colors"
              title="View on Solscan"
              aria-label="View on Solscan"
            >
              <ExternalLink size={13} />
            </a>
          </div>
        </div>

        {/* Escrow config (if available) */}
        {escrowConfig && (
          <div className="rounded-xl border border-border bg-bg-surface p-5">
            <h2 className="text-sm font-semibold text-text-primary mb-3">
              Escrow Configuration
            </h2>
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 text-sm">
              <div>
                <p className="text-xs text-text-secondary">Network</p>
                <p className="font-medium text-text-primary">{escrowConfig.network}</p>
              </div>
              <div>
                <p className="text-xs text-text-secondary">Current Slot</p>
                <p className="font-medium text-text-primary tabular-nums">{formatNumber(escrowConfig.current_slot)}</p>
              </div>
              <div>
                <p className="text-xs text-text-secondary">USDC Mint</p>
                <code className="text-xs font-mono text-text-secondary">{escrowConfig.usdc_mint}</code>
              </div>
              <div>
                <p className="text-xs text-text-secondary">Program ID</p>
                <code className="text-xs font-mono text-text-secondary">{escrowConfig.escrow_program_id}</code>
              </div>
            </div>
          </div>
        )}

        {/* Top wallets table (from API) or recent transactions (mock fallback) */}
        {topWallets ? (
          <div className="rounded-xl border border-border bg-bg-surface overflow-hidden">
            <div className="px-5 py-4 border-b border-border-subtle">
              <h2 className="text-sm font-semibold text-text-primary">
                Top Wallets
              </h2>
            </div>
            <div className="divide-y divide-border-subtle">
              {topWallets.map((w) => (
                <div
                  key={w.wallet}
                  className="flex items-center gap-4 px-5 py-3 hover:bg-bg-surface-hover transition-colors"
                >
                  <code className="text-xs font-mono text-text-secondary min-w-0 truncate">
                    {w.wallet}
                  </code>
                  <div className="ml-auto flex items-center gap-6">
                    <span className="text-sm font-medium text-text-primary tabular-nums">
                      {formatUSDC(w.cost, 4)}
                    </span>
                    <span className="text-xs text-text-tertiary tabular-nums">
                      {formatNumber(w.requests)} req
                    </span>
                  </div>
                </div>
              ))}
            </div>
          </div>
        ) : (
          /* Fallback: mock transaction history */
          <div className="rounded-xl border border-border bg-bg-surface overflow-hidden">
            <div className="px-5 py-4 border-b border-border-subtle flex items-center justify-between">
              <h2 className="text-sm font-semibold text-text-primary">
                Recent Transactions
              </h2>
              <a
                href={`https://solscan.io/account/${addr}`}
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-1 text-xs text-info hover:underline"
              >
                View all <ExternalLink size={10} />
              </a>
            </div>
            <div className="divide-y divide-border-subtle">
              {WALLET_TXS.map((tx) => (
                <div
                  key={tx.signature}
                  className="flex items-center gap-4 px-5 py-3 hover:bg-bg-surface-hover transition-colors"
                >
                  <div className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-full bg-brand-subtle text-brand-text">
                    <ArrowUpRight size={14} />
                  </div>
                  <div className="flex-1 min-w-0">
                    <p className="text-sm font-medium text-text-primary truncate">
                      {tx.model}
                    </p>
                    <p className="text-xs text-text-tertiary">
                      {tx.timestamp} ·{" "}
                      <a
                        href={`https://solscan.io/tx/${tx.signature}`}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="hover:underline text-info"
                      >
                        {tx.signature}
                      </a>
                    </p>
                  </div>
                  <div className="text-right">
                    <p className="text-sm font-medium text-text-primary tabular-nums">
                      −${tx.amount}
                    </p>
                    <p className="text-xs text-text-tertiary">USDC</p>
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
        <div className="rounded-xl border border-info/20 bg-info/10 p-5">
          <h2 className="text-sm font-semibold text-info mb-2">
            Fund Your Wallet
          </h2>
          <ol className="space-y-1.5 text-sm text-info list-decimal list-inside">
            <li>
              Send USDC-SPL to your wallet address above on Solana mainnet
            </li>
            <li>
              USDC Mint:{" "}
              <code className="font-mono text-xs bg-info/20 rounded px-1 py-0.5">
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
