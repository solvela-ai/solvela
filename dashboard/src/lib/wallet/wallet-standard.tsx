"use client";

// ---------------------------------------------------------------------------
// Solana Wallet Standard connect provider (interop-first, no proprietary
// connector). Discovers any installed Wallet-Standard wallet (Phantom,
// Solflare, Backpack, embedded wallets from Privy/Dynamic, …) via the
// `@wallet-standard/app` registry, lets the user connect one, and exposes a
// byte-faithful `signAndSend`.
//
// Design rule (money-path adjacent): we feed the wallet the EXACT unsigned
// message bytes the gateway returned and that the user has already verified
// (see `lib/escrow/verify`). We do NOT re-compile or re-order the message — the
// wallet signs precisely the bytes that were verified. The message is wrapped
// into the legacy wire transaction `compact-u16(1) || 64 zero bytes || message`;
// the wallet fills the single fee-payer signature slot (account index 0 = the
// connected wallet) and broadcasts.
// ---------------------------------------------------------------------------

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

import { getWallets } from "@wallet-standard/app";
import type { Wallet, WalletAccount } from "@wallet-standard/base";
import {
  StandardConnect,
  StandardDisconnect,
  StandardEvents,
  type StandardConnectFeature,
  type StandardDisconnectFeature,
  type StandardEventsFeature,
} from "@wallet-standard/features";
import {
  SolanaSignAndSendTransaction,
  type SolanaSignAndSendTransactionFeature,
} from "@solana/wallet-standard-features";
import { getBase58Decoder } from "@solana/kit";

/** A Wallet-Standard cluster identifier, e.g. `solana:mainnet` / `solana:devnet`. */
export type SolanaChain = `solana:${string}`;

/**
 * Target cluster for signing/broadcast. The gateway's deposit-tx `network`
 * field is a hardcoded mainnet genesis constant and is NOT a reliable cluster
 * signal, so the dashboard picks its own cluster from env (defaulting to
 * mainnet). For local testing against a devnet gateway, set
 * `NEXT_PUBLIC_SOLANA_CHAIN=solana:devnet`.
 */
export const SOLANA_CHAIN: SolanaChain =
  (process.env.NEXT_PUBLIC_SOLANA_CHAIN as SolanaChain | undefined) ??
  "solana:mainnet";

/** A wallet we can actually use: it can connect AND sign+send Solana txs. */
function isUsableWallet(wallet: Wallet): boolean {
  return (
    StandardConnect in wallet.features &&
    SolanaSignAndSendTransaction in wallet.features
  );
}

function connectFeature(wallet: Wallet) {
  return (wallet.features as StandardConnectFeature)[StandardConnect];
}
function disconnectFeature(wallet: Wallet) {
  return (wallet.features as Partial<StandardDisconnectFeature>)[
    StandardDisconnect
  ];
}
function eventsFeature(wallet: Wallet) {
  return (wallet.features as Partial<StandardEventsFeature>)[StandardEvents];
}
function signAndSendFeature(wallet: Wallet) {
  return (wallet.features as SolanaSignAndSendTransactionFeature)[
    SolanaSignAndSendTransaction
  ];
}

export interface ConnectedWallet {
  wallet: Wallet;
  account: WalletAccount;
}

export interface SolanaWalletContextValue {
  /** Installed, usable wallets discovered via the Wallet-Standard registry. */
  wallets: Wallet[];
  /** The connected wallet + account, or null. */
  connected: ConnectedWallet | null;
  connecting: boolean;
  /** Last connect failure (e.g. the user declined), or null. */
  connectError: string | null;
  /** The cluster used for signing/broadcast. */
  chain: SolanaChain;
  connect: (wallet: Wallet) => Promise<void>;
  disconnect: () => Promise<void>;
  /**
   * Sign + broadcast the given UNSIGNED legacy-message bytes verbatim. Returns
   * the base58 transaction signature. Throws if not connected or the wallet
   * rejects/fails.
   */
  signAndSend: (messageBytes: Uint8Array) => Promise<string>;
}

const SolanaWalletContext = createContext<SolanaWalletContextValue | null>(null);

export function SolanaWalletProvider({ children }: { children: ReactNode }) {
  const [wallets, setWallets] = useState<Wallet[]>([]);
  const [connected, setConnected] = useState<ConnectedWallet | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);

  // Discover wallets on the client only (the registry touches `window`). Keep
  // the list in sync with register/unregister events.
  useEffect(() => {
    const registry = getWallets();
    const refresh = () =>
      setWallets(registry.get().filter(isUsableWallet).slice());
    refresh();
    const offRegister = registry.on("register", refresh);
    const offUnregister = registry.on("unregister", refresh);
    return () => {
      offRegister();
      offUnregister();
    };
  }, []);

  const connect = useCallback(async (wallet: Wallet) => {
    setConnecting(true);
    setConnectError(null);
    try {
      const { accounts } = await connectFeature(wallet).connect();
      const account =
        accounts.find((a) => a.chains.includes(SOLANA_CHAIN)) ?? accounts[0];
      if (!account) {
        throw new Error("Wallet returned no accounts");
      }
      setConnected({ wallet, account });
    } catch (e) {
      // Surface the failure (e.g. the user declined the connection in their
      // wallet) instead of letting the rejection vanish into a `void connect()`
      // — otherwise the UI just silently returns to the wallet list.
      setConnectError(e instanceof Error ? e.message : "Failed to connect wallet");
    } finally {
      setConnecting(false);
    }
  }, []);

  const disconnect = useCallback(async () => {
    const current = connected;
    setConnected(null);
    if (current) {
      await disconnectFeature(current.wallet)?.disconnect();
    }
  }, [connected]);

  // Track wallet-side account changes / disconnects for the connected wallet.
  useEffect(() => {
    if (!connected) return;
    const events = eventsFeature(connected.wallet);
    if (!events) return;
    const off = events.on("change", (props) => {
      if (props.accounts) {
        const next =
          props.accounts.find((a) => a.chains.includes(SOLANA_CHAIN)) ??
          props.accounts[0] ??
          null;
        // Functional updater: read the wallet from current state, never the
        // closed-over `connected`, so a rapid wallet switch can't pin a stale
        // wallet to a new account.
        setConnected((prev) =>
          prev && next ? { wallet: prev.wallet, account: next } : null,
        );
      }
    });
    return off;
  }, [connected]);

  const signAndSend = useCallback(
    async (messageBytes: Uint8Array): Promise<string> => {
      if (!connected) {
        throw new Error("No wallet connected");
      }
      // Wrap the verified message verbatim into an unsigned legacy wire tx:
      // compact-u16(1) signature || 64 zero signature bytes || message. The
      // message bytes are NOT modified, so the wallet signs exactly what the
      // user verified.
      const tx = new Uint8Array(1 + 64 + messageBytes.length);
      tx[0] = 1;
      tx.set(messageBytes, 65);

      const [output] = await signAndSendFeature(
        connected.wallet,
      ).signAndSendTransaction({
        account: connected.account,
        transaction: tx,
        chain: SOLANA_CHAIN,
      });
      return getBase58Decoder().decode(output.signature);
    },
    [connected],
  );

  const value = useMemo<SolanaWalletContextValue>(
    () => ({
      wallets,
      connected,
      connecting,
      connectError,
      chain: SOLANA_CHAIN,
      connect,
      disconnect,
      signAndSend,
    }),
    [wallets, connected, connecting, connectError, connect, disconnect, signAndSend],
  );

  return (
    <SolanaWalletContext.Provider value={value}>
      {children}
    </SolanaWalletContext.Provider>
  );
}

export function useSolanaWallet(): SolanaWalletContextValue {
  const ctx = useContext(SolanaWalletContext);
  if (!ctx) {
    throw new Error(
      "useSolanaWallet must be used within a <SolanaWalletProvider>",
    );
  }
  return ctx;
}
