import { Copy, ExternalLink, ArrowUpRight, AlertTriangle } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { StatusDot } from "@/components/ui/status-dot";
import { WALLET_TXS, DASHBOARD_STATS } from "@/lib/mock-data";
import { fetchAdminStats, fetchEscrowConfig } from "@/lib/api";
import { formatUSDC, formatNumber } from "@/lib/utils";

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

      <div className="flex-1 p-6 space-y-5">
        {/* Mock data warning */}
        {usingMockData && (
          <div className="flex items-center gap-2 rounded border border-border px-4 py-2.5 text-sm text-text-secondary">
            <AlertTriangle size={13} className="flex-shrink-0 text-warning" />
            <span>Gateway offline — showing sample data.</span>
          </div>
        )}

        {/* Balance */}
        <div className="terminal-card">
          <div className="terminal-card-titlebar">
            <span className="terminal-card-dots">
              <span className="terminal-card-dot" />
              <span className="terminal-card-dot" />
              <span className="terminal-card-dot" />
            </span>
            <span>wallet.recipient</span>
            <span className="ml-auto"><StatusDot status="ok" label="Connected" /></span>
          </div>
          <div className="terminal-card-screen">
            <p
              className="tabular-nums leading-none"
              style={{
                fontFamily: 'var(--font-serif)',
                fontSize: '36px',
                fontWeight: 500,
                color: 'var(--heading-color)',
                letterSpacing: '-0.02em',
              }}
            >
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
                className="rounded border border-border p-1.5 text-text-tertiary hover:text-text-primary hover:bg-bg-surface transition-colors"
                title="Copy address"
                aria-label="Copy address"
              >
                <Copy size={12} />
              </button>
              <a
                href={`https://solscan.io/account/${addr}`}
                target="_blank"
                rel="noopener noreferrer"
                className="rounded border border-border p-1.5 text-text-tertiary hover:text-text-primary hover:bg-bg-surface transition-colors"
                title="View on Solscan"
                aria-label="View on Solscan"
              >
                <ExternalLink size={12} />
              </a>
            </div>
          </div>
        </div>

        {/* Escrow config */}
        {escrowConfig && (
          <div className="terminal-card">
            <div className="terminal-card-titlebar">
              <span className="terminal-card-dots">
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot" />
              </span>
              <span>wallet.escrow</span>
            </div>
            <div className="terminal-card-screen">
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                <div className="rounded border border-border p-3" style={{ background: 'var(--sidebar-bg)' }}>
                  <p className="text-xs text-text-tertiary font-mono mb-1">Network</p>
                  <p className="text-sm font-medium text-text-primary">{escrowConfig.network}</p>
                </div>
                <div className="rounded border border-border p-3" style={{ background: 'var(--sidebar-bg)' }}>
                  <p className="text-xs text-text-tertiary font-mono mb-1">Current Slot</p>
                  <p className="text-sm font-medium text-text-primary tabular-nums font-mono">{formatNumber(escrowConfig.current_slot)}</p>
                </div>
                <div className="rounded border border-border p-3" style={{ background: 'var(--sidebar-bg)' }}>
                  <p className="text-xs text-text-tertiary font-mono mb-1">USDC Mint</p>
                  <code className="text-xs font-mono text-text-secondary break-all">{escrowConfig.usdc_mint}</code>
                </div>
                <div className="rounded border border-border p-3" style={{ background: 'var(--sidebar-bg)' }}>
                  <p className="text-xs text-text-tertiary font-mono mb-1">Program ID</p>
                  <code className="text-xs font-mono text-text-secondary break-all">{escrowConfig.escrow_program_id}</code>
                </div>
              </div>
            </div>
          </div>
        )}

        {/* Transactions / wallets table */}
        {topWallets ? (
          <div className="terminal-card overflow-hidden">
            <div className="terminal-card-titlebar">
              <span className="terminal-card-dots">
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot" />
              </span>
              <span>wallet.deposits</span>
            </div>
            <div className="divide-y divide-border" style={{ background: 'var(--popover)' }}>
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
          </div>
        ) : (
          <div className="terminal-card overflow-hidden">
            <div className="terminal-card-titlebar">
              <span className="terminal-card-dots">
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot" />
              </span>
              <span>wallet.deposits</span>
              <a
                href={`https://solscan.io/account/${addr}`}
                target="_blank"
                rel="noopener noreferrer"
                className="ml-auto flex items-center gap-1 text-text-tertiary hover:text-text-primary transition-colors"
                style={{ fontSize: '10px' }}
              >
                View all <ExternalLink size={10} />
              </a>
            </div>
            <div className="divide-y divide-border" style={{ background: 'var(--popover)' }}>
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
          </div>
        )}

        {/* Fund instructions */}
        <div className="terminal-card">
          <div className="terminal-card-titlebar">
            <span className="terminal-card-dots">
              <span className="terminal-card-dot" />
              <span className="terminal-card-dot" />
              <span className="terminal-card-dot" />
            </span>
            <span>wallet.fund</span>
          </div>
          <div className="terminal-card-screen">
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
          </div>
        </div>
      </div>
    </div>
  );
}
