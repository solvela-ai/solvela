"use client";

// ---------------------------------------------------------------------------
// BYO-wallet escrow funding (dashboard PR 2).
//
// Connect any Wallet-Standard wallet, request an UNSIGNED escrow-deposit
// message from the gateway (`POST /v1/escrow/deposit-tx`, #597), VERIFY what
// you're about to sign (decode + independently re-derive the destination), then
// sign + broadcast. The gateway holds no key on this path; the user's wallet is
// the sole signer and fee payer.
//
// Money-path safety: the deposit is NEVER signed until `verifyDepositMessage`
// confirms the message decodes to exactly one `deposit` instruction whose escrow
// PDA, re-derived from the *connected* pubkey, matches the message and the
// gateway's stated intent. A failed verification hard-stops before any signature
// is requested.
// ---------------------------------------------------------------------------

import { useCallback, useMemo, useState } from "react";
import {
  Wallet as WalletIcon,
  ShieldCheck,
  Check,
  AlertTriangle,
  ExternalLink,
  RefreshCw,
  Loader,
  X,
} from "lucide-react";

import { getBase64Decoder, getBase64Encoder } from "@solana/kit";

import { TerminalCard } from "@/components/ui/terminal-card";
import {
  SolanaWalletProvider,
  useSolanaWallet,
  type SolanaChain,
} from "@/lib/wallet/wallet-standard";
import { usdcDecimalToAtomic } from "@/lib/escrow/amount";
import { requestDepositTx } from "@/lib/escrow/deposit-client";
import { verifyDepositMessage, type VerifiedDeposit } from "@/lib/escrow/verify";

export interface EscrowFundingProps {
  /** Browser-reachable gateway base URL (NEXT_PUBLIC_GATEWAY_URL). */
  gatewayUrl: string;
  /** Whether the gateway has an escrow program configured. */
  escrowConfigured: boolean;
}

const PRIMARY_BTN =
  "inline-flex items-center justify-center gap-2 rounded border border-[color:var(--accent-salmon)] bg-[color:var(--accent-salmon)] px-4 py-2 text-sm font-medium text-[#1a1410] hover:bg-[color:var(--accent-salmon-hover)] disabled:opacity-50 disabled:cursor-not-allowed transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[color:var(--accent-salmon)]";

const SECONDARY_BTN =
  "inline-flex items-center justify-center gap-2 rounded border border-border px-4 py-2 text-sm font-medium text-text-secondary hover:text-text-primary hover:bg-bg-surface disabled:opacity-50 disabled:cursor-not-allowed transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[color:var(--accent-salmon)]";

const INPUT_CLS =
  "w-full rounded border border-border px-3 py-2 text-sm text-text-primary placeholder-text-tertiary bg-bg-inset focus:border-foreground focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[color:var(--accent-salmon)] transition-colors font-mono";

function shorten(addr: string, head = 6, tail = 6): string {
  return addr.length > head + tail + 1
    ? `${addr.slice(0, head)}…${addr.slice(-tail)}`
    : addr;
}

/** A fresh random 32-byte service_id seeding the escrow PDA. */
function randomServiceId(): { bytes: Uint8Array; b64: string } {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return { bytes, b64: getBase64Decoder().decode(bytes) };
}

/** Solscan cluster query suffix for the configured chain. */
function explorerCluster(chain: SolanaChain): string {
  if (chain === "solana:mainnet") return "";
  if (chain === "solana:devnet") return "?cluster=devnet";
  if (chain === "solana:testnet") return "?cluster=testnet";
  return "";
}

function errMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// ---------------------------------------------------------------------------
// Connect-wallet sub-panel
// ---------------------------------------------------------------------------

function ConnectPanel() {
  const { wallets, connected, connecting, connectError, connect, disconnect } =
    useSolanaWallet();

  if (connected) {
    return (
      <div className="flex items-center gap-3">
        {connected.wallet.icon && (
          // eslint-disable-next-line @next/next/no-img-element
          <img
            src={connected.wallet.icon}
            alt=""
            className="h-6 w-6 rounded"
            aria-hidden
          />
        )}
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium text-text-primary">
            {connected.wallet.name}
          </p>
          <code className="text-xs font-mono text-text-tertiary">
            {shorten(connected.account.address, 8, 8)}
          </code>
        </div>
        <button
          type="button"
          onClick={() => void disconnect()}
          className={SECONDARY_BTN}
        >
          <X size={13} /> Disconnect
        </button>
      </div>
    );
  }

  if (wallets.length === 0) {
    return (
      <div className="text-sm text-text-secondary">
        No Solana wallet detected. Install{" "}
        <a
          href="https://phantom.app/"
          target="_blank"
          rel="noopener noreferrer"
          className="text-[color:var(--accent-salmon)] hover:underline"
        >
          Phantom
        </a>
        ,{" "}
        <a
          href="https://solflare.com/"
          target="_blank"
          rel="noopener noreferrer"
          className="text-[color:var(--accent-salmon)] hover:underline"
        >
          Solflare
        </a>
        , or{" "}
        <a
          href="https://backpack.app/"
          target="_blank"
          rel="noopener noreferrer"
          className="text-[color:var(--accent-salmon)] hover:underline"
        >
          Backpack
        </a>{" "}
        and reload.
      </div>
    );
  }

  return (
    <div className="flex flex-wrap gap-2">
      {wallets.map((w) => (
        <button
          key={w.name}
          type="button"
          disabled={connecting}
          onClick={() => void connect(w)}
          className={SECONDARY_BTN}
        >
          {w.icon ? (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={w.icon} alt="" className="h-4 w-4 rounded" aria-hidden />
          ) : (
            <WalletIcon size={14} />
          )}
          {w.name}
        </button>
      ))}
      {connectError && (
        <p className="w-full text-xs text-error">{connectError}</p>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Verify-what-you-sign review panel
// ---------------------------------------------------------------------------

function IntentRow({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt className="text-[11px] text-text-tertiary font-mono uppercase tracking-wide mb-1">
        {label}
      </dt>
      <dd>
        <code className="text-xs font-mono text-text-secondary break-all">
          {value}
        </code>
      </dd>
    </div>
  );
}

function ReviewPanel({
  deposit,
  chain,
  onConfirm,
  onCancel,
  signing,
}: {
  deposit: VerifiedDeposit;
  chain: SolanaChain;
  onConfirm: () => void;
  onCancel: () => void;
  signing: boolean;
}) {
  return (
    <div className="space-y-4">
      <div className="flex items-start gap-2 rounded border border-border bg-bg-surface px-3 py-2.5">
        <ShieldCheck
          size={14}
          className="mt-0.5 flex-shrink-0 text-[color:var(--accent-salmon)]"
        />
        <p className="text-xs text-text-secondary">
          Verified locally: this transaction is a single escrow{" "}
          <span className="font-medium text-text-primary">deposit</span> whose
          destination was re-derived from your connected wallet. Review the
          decoded intent before signing.
        </p>
      </div>

      <p className="metric-xl">{deposit.amountUsdc} USDC</p>
      <p className="-mt-1 text-xs text-text-tertiary font-mono">
        {deposit.amountAtomic.toString()} atomic micro-USDC · {chain}
      </p>

      <dl className="grid grid-cols-1 sm:grid-cols-2 gap-x-8 gap-y-4">
        <IntentRow label="Escrow PDA (destination)" value={deposit.escrowPda} />
        <IntentRow label="Vault ATA" value={deposit.vaultAta} />
        <IntentRow label="Escrow program" value={deposit.programId} />
        <IntentRow label="USDC mint" value={deposit.usdcMint} />
        <IntentRow label="Provider" value={deposit.provider} />
        <IntentRow
          label="Expiry slot"
          value={deposit.expirySlot.toLocaleString()}
        />
      </dl>

      <div className="flex items-center gap-2 pt-1">
        <button
          type="button"
          onClick={onConfirm}
          disabled={signing}
          className={PRIMARY_BTN}
        >
          {signing ? (
            <>
              <Loader size={14} className="animate-spin" /> Signing…
            </>
          ) : (
            <>
              <ShieldCheck size={14} /> Confirm &amp; sign deposit
            </>
          )}
        </button>
        <button
          type="button"
          onClick={onCancel}
          disabled={signing}
          className={SECONDARY_BTN}
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main funding flow
// ---------------------------------------------------------------------------

type Status = "idle" | "building" | "review" | "signing" | "success" | "error";

function FundingFlow({ gatewayUrl, escrowConfigured }: EscrowFundingProps) {
  const { connected, chain, signAndSend } = useSolanaWallet();

  const [amount, setAmount] = useState("");
  const [amountError, setAmountError] = useState<string | null>(null);
  const [service, setService] = useState(() => randomServiceId());
  const [status, setStatus] = useState<Status>("idle");
  const [error, setError] = useState<string | null>(null);
  const [review, setReview] = useState<{
    deposit: VerifiedDeposit;
    messageB64: string;
  } | null>(null);
  const [signature, setSignature] = useState<string | null>(null);

  const busy = status === "building" || status === "signing";

  const reset = useCallback(() => {
    setStatus("idle");
    setError(null);
    setReview(null);
    setSignature(null);
    setAmount("");
    setAmountError(null);
    setService(randomServiceId());
  }, []);

  const handleBuild = useCallback(async () => {
    setError(null);
    setAmountError(null);

    let amountAtomic: string;
    try {
      amountAtomic = usdcDecimalToAtomic(amount);
    } catch (e) {
      setAmountError(errMessage(e));
      return;
    }
    if (!connected) {
      setError("Connect a wallet first.");
      return;
    }

    setStatus("building");
    try {
      const resp = await requestDepositTx(gatewayUrl, {
        agent_wallet: connected.account.address,
        service_id: service.b64,
        amount: amountAtomic,
      });

      const verdict = await verifyDepositMessage({
        messageB64: resp.message,
        decodedIntent: resp.decoded_intent,
        expected: {
          agentWallet: connected.account.address,
          amountAtomic,
          serviceIdB64: service.b64,
        },
      });

      if (!verdict.ok) {
        setStatus("error");
        setError(`Verification failed — refusing to sign. ${verdict.reason}`);
        return;
      }

      setReview({ deposit: verdict.deposit, messageB64: resp.message });
      setStatus("review");
    } catch (e) {
      setStatus("error");
      setError(errMessage(e));
    }
  }, [amount, connected, gatewayUrl, service.b64]);

  const handleSign = useCallback(async () => {
    if (!review) return;
    setStatus("signing");
    setError(null);
    try {
      // Sign exactly the bytes that were verified: re-decoding the same base64
      // string is deterministic, so these bytes are identical to those
      // `verifyDepositMessage` inspected.
      // NOTE on @solana/kit naming: a base64 *Encoder*'s `.encode(string)`
      // returns the raw bytes the base64 represents — i.e. this line DECODES
      // base64 → bytes. (Conversely getBase64Decoder().decode(bytes) → string.)
      // Do not "fix" this to getBase64Decoder; the direction here is correct.
      const messageBytes = new Uint8Array(
        getBase64Encoder().encode(review.messageB64),
      );
      const sig = await signAndSend(messageBytes);
      setSignature(sig);
      setStatus("success");
    } catch (e) {
      setStatus("error");
      setError(errMessage(e));
    }
  }, [review, signAndSend]);

  if (!escrowConfigured) {
    return (
      <div className="flex items-center gap-2 text-sm text-text-secondary">
        <AlertTriangle size={13} className="flex-shrink-0 text-warning" />
        <span>
          Escrow is not configured on this gateway, so funding is unavailable.
        </span>
      </div>
    );
  }

  if (status === "success" && signature) {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2 text-sm text-text-primary">
          <span className="flex h-7 w-7 items-center justify-center rounded-full border border-success/40 text-success">
            <Check size={14} />
          </span>
          Deposit submitted.
        </div>
        {review && (
          <p className="text-sm text-text-secondary">
            Funded escrow{" "}
            <code className="font-mono text-xs text-text-primary break-all">
              {review.deposit.escrowPda}
            </code>{" "}
            with {review.deposit.amountUsdc} USDC.
          </p>
        )}
        <a
          href={`https://solscan.io/tx/${signature}${explorerCluster(chain)}`}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-1.5 text-sm text-[color:var(--accent-salmon)] hover:underline font-mono"
        >
          {shorten(signature, 10, 10)} <ExternalLink size={12} />
        </a>
        <div>
          <button type="button" onClick={reset} className={SECONDARY_BTN}>
            <RefreshCw size={13} /> Fund another
          </button>
        </div>
      </div>
    );
  }

  if ((status === "review" || status === "signing") && review) {
    return (
      <ReviewPanel
        deposit={review.deposit}
        chain={chain}
        signing={status === "signing"}
        onConfirm={() => void handleSign()}
        onCancel={reset}
      />
    );
  }

  // idle / building / error → the entry form.
  return (
    <div className="space-y-4">
      <div>
        <label
          htmlFor="deposit-amount"
          className="block text-[11px] text-text-tertiary font-mono uppercase tracking-wide mb-1.5"
        >
          Amount (USDC)
        </label>
        <input
          id="deposit-amount"
          inputMode="decimal"
          autoComplete="off"
          placeholder="5.00"
          value={amount}
          disabled={busy}
          onChange={(e) => {
            setAmount(e.target.value);
            setAmountError(null);
          }}
          className={INPUT_CLS}
        />
        {amountError && (
          <p className="mt-1.5 text-xs text-error">{amountError}</p>
        )}
        <p className="mt-1.5 text-xs text-text-tertiary font-mono break-all">
          escrow service_id: {shorten(service.b64, 10, 8)}
          <button
            type="button"
            onClick={() => setService(randomServiceId())}
            disabled={busy}
            className="ml-2 inline-flex items-center gap-1 text-text-tertiary hover:text-text-primary transition-colors disabled:opacity-50"
            title="Generate a new escrow id"
          >
            <RefreshCw size={10} /> new
          </button>
        </p>
      </div>

      {error && (
        <div className="flex items-start gap-2 rounded border border-error/40 bg-error/5 px-3 py-2.5 text-xs text-text-secondary">
          <AlertTriangle size={13} className="mt-0.5 flex-shrink-0 text-error" />
          <span>{error}</span>
        </div>
      )}

      <button
        type="button"
        onClick={() => void handleBuild()}
        disabled={busy || !connected || amount.trim() === ""}
        className={PRIMARY_BTN}
      >
        {status === "building" ? (
          <>
            <Loader size={14} className="animate-spin" /> Building…
          </>
        ) : (
          <>
            <ShieldCheck size={14} /> Review deposit
          </>
        )}
      </button>
      {!connected && (
        <p className="text-xs text-text-tertiary">
          Connect a wallet above to fund an escrow.
        </p>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Public component (wires the wallet provider)
// ---------------------------------------------------------------------------

export function EscrowFunding(props: EscrowFundingProps) {
  const subtitle = useMemo(
    () =>
      props.escrowConfigured
        ? "Connect a wallet you control and fund an escrow — verified before you sign."
        : "Escrow funding is unavailable on this gateway.",
    [props.escrowConfigured],
  );

  return (
    <SolanaWalletProvider>
      <TerminalCard
        title="Fund an escrow with your wallet"
        accentDot
        meta={
          <span className="text-text-tertiary truncate text-xxs">
            Wallet Standard · non-custodial
          </span>
        }
      >
        <div className="space-y-5">
          <p className="text-xs text-text-tertiary">{subtitle}</p>
          <ConnectPanel />
          <div className="h-px bg-border" />
          <FundingFlow {...props} />
        </div>
      </TerminalCard>
    </SolanaWalletProvider>
  );
}
