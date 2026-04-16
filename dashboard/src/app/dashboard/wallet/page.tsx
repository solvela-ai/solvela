import { Copy, ExternalLink, ArrowUpRight, AlertTriangle } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { StatusDot } from "@/components/ui/status-dot";
import { TerminalCard } from "@/components/ui/terminal-card";
import { WALLET_TXS, DASHBOARD_STATS } from "@/lib/mock-data";
import { fetchAdminStats, fetchEscrowConfig } from "@/lib/api";
import { formatUSDC, formatNumber } from "@/lib/utils";

const RECIPIENT_WALLET =
  process.env.SOLVELA_SOLANA_RECIPIENT_WALLET ??
  process.env.RCR_SOLANA_RECIPIENT_WALLET ??
  "Configure SOLVELA_SOLANA_RECIPIENT_WALLET in .env";

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

      <div className="flex-1 p-6 space-y-5">
        {/* Mock data warning */}
        {usingMockData && (
          <div role="status" aria-live="polite" className="flex items-center gap-2 rounded border border-border px-4 py-2.5 text-sm text-text-secondary">
            <AlertTriangle size={13} className="flex-shrink-0 text-warning" />
            <span>Gateway offline — showing sample data.</span>
          </div>
        )}

        {/* Balance */}
        <TerminalCard title="Recipient wallet" meta={<StatusDot status="ok" label="Connected" />}>
            <p className="metric-xl">
              {formatUSDC(totalSpend, 2)}
            </p>
            <p className="mt-1.5 text-xs text-text-tertiary font-mono">
              USDC on Solana mainnet · last 30 days
            </p>
            <div className="mt-4 flex items-center gap-2">
              <code className="rounded border border-border bg-bg-inset px-3 py-1.5 text-xs font-mono text-text-secondary">
                {short}
              </code>
              <button
                type="button"
                className="flex h-10 w-10 items-center justify-center rounded border border-border text-text-tertiary hover:text-text-primary hover:bg-bg-surface transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[color:var(--accent-salmon)]"
                title="Copy address"
                aria-label="Copy address"
              >
                <Copy size={12} />
              </button>
              <a
                href={`https://solscan.io/account/${addr}`}
                target="_blank"
                rel="noopener noreferrer"
                className="flex h-10 w-10 items-center justify-center rounded border border-border text-text-tertiary hover:text-text-primary hover:bg-bg-surface transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[color:var(--accent-salmon)]"
                title="View on Solscan"
                aria-label="View on Solscan"
              >
                <ExternalLink size={12} />
              </a>
            </div>
        </TerminalCard>

        {/* Escrow config */}
        {escrowConfig && (
          <TerminalCard title="Escrow configuration">
            <dl className="grid grid-cols-1 sm:grid-cols-2 gap-x-8 gap-y-5">
              <div>
                <dt className="text-[11px] text-text-tertiary font-mono uppercase tracking-wide mb-1">Network</dt>
                <dd className="text-sm font-medium text-text-primary">{escrowConfig.network}</dd>
              </div>
              <div>
                <dt className="text-[11px] text-text-tertiary font-mono uppercase tracking-wide mb-1">Current Slot</dt>
                <dd className="text-sm font-medium text-text-primary tabular-nums font-mono">{formatNumber(escrowConfig.current_slot)}</dd>
              </div>
              <div>
                <dt className="text-[11px] text-text-tertiary font-mono uppercase tracking-wide mb-1">USDC Mint</dt>
                <dd><code className="text-xs font-mono text-text-secondary break-all">{escrowConfig.usdc_mint}</code></dd>
              </div>
              <div>
                <dt className="text-[11px] text-text-tertiary font-mono uppercase tracking-wide mb-1">Program ID</dt>
                <dd><code className="text-xs font-mono text-text-secondary break-all">{escrowConfig.escrow_program_id}</code></dd>
              </div>
            </dl>
          </TerminalCard>
        )}

        {/* Transactions / wallets table */}
        {topWallets ? (
          <TerminalCard title="wallet.deposits" bare className="overflow-hidden">
            <div className="divide-y divide-border bg-popover">
              {topWallets.map((w) => (
                <div
                  key={w.wallet}
                  className="flex items-center gap-4 px-5 py-3 hover:bg-bg-surface transition-colors"
                >
                  <code className="text-xs font-mono text-text-secondary min-w-0 truncate">
                    {w.wallet}
                  </code>
                  <div className="ml-auto flex items-center gap-6">
                    <span className="text-sm font-medium text-text-primary tabular-nums font-mono">
                      {formatUSDC(w.cost, 4)}
                    </span>
                    <span className="text-xs text-text-tertiary tabular-nums font-mono">
                      {formatNumber(w.requests)} req
                    </span>
                  </div>
                </div>
              ))}
            </div>
          </TerminalCard>
        ) : (
          <TerminalCard
            title="wallet.deposits"
            meta={
              <a
                href={`https://solscan.io/account/${addr}`}
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-1 text-text-tertiary hover:text-text-primary transition-colors text-xxs"
              >
                View all <ExternalLink size={10} />
              </a>
            }
            bare
            className="overflow-hidden"
          >
            <div className="divide-y divide-border bg-popover">
              {WALLET_TXS.map((tx) => (
                <div
                  key={tx.signature}
                  className="flex items-center gap-4 px-5 py-3 hover:bg-bg-surface transition-colors"
                >
                  <div className="flex h-7 w-7 flex-shrink-0 items-center justify-center rounded border border-border text-text-tertiary">
                    <ArrowUpRight size={13} />
                  </div>
                  <div className="flex-1 min-w-0">
                    <p className="text-xs font-medium text-text-primary truncate font-mono">
                      {tx.model}
                    </p>
                    <p className="text-xs text-text-tertiary font-mono">
                      {tx.timestamp} ·{" "}
                      <a
                        href={`https://solscan.io/tx/${tx.signature}`}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="hover:text-text-primary transition-colors"
                      >
                        {tx.signature}
                      </a>
                    </p>
                  </div>
                  <div className="text-right">
                    <p className="text-xs font-medium text-text-primary tabular-nums font-mono">
                      −${tx.amount}
                    </p>
                    <p className="text-xs text-text-tertiary font-mono">USDC</p>
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
          </TerminalCard>
        )}

        {/* Fund instructions */}
        <TerminalCard title="Fund your wallet">
          <ol className="space-y-1.5 text-sm text-text-secondary list-decimal list-inside">
            <li>Send USDC-SPL to your wallet address above on Solana mainnet</li>
            <li>
              USDC Mint:{" "}
              <code className="font-mono text-xs border border-border rounded px-1 py-0.5 text-text-tertiary">
                EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v
              </code>
            </li>
            <li>Your wallet must hold a small amount of SOL for transaction fees (~0.002 SOL)</li>
            <li>Payments are deducted automatically per API call via x402 protocol</li>
          </ol>
        </TerminalCard>
      </div>
    </div>
  );
}
