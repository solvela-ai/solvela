/**
 * Configuration for the @rustyclaw/rcr OpenClaw plugin.
 *
 * Reads from the same env vars already present on all tenant VPSes:
 *   LLM_ROUTER_API_URL     — Solvela gateway base URL
 *   LLM_ROUTER_WALLET_KEY  — Base58 Solana private key for x402 payments
 */
export interface RcrConfig {
  /** Solvela gateway base URL (no trailing slash). */
  gatewayUrl: string;
  /** Base58-encoded Solana private key for signing x402 payments. */
  walletKey: string;
  /**
   * Default model to route requests to.
   * "auto" lets the RCR smart router pick the cheapest capable model.
   */
  defaultModel: string;
}

export class ConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ConfigError';
  }
}

/**
 * Loads and validates plugin configuration from environment variables.
 * Throws ConfigError if required vars are missing.
 */
export function loadConfig(overrides: Partial<RcrConfig> = {}): RcrConfig {
  const gatewayUrl = (
    overrides.gatewayUrl ||
    process.env.LLM_ROUTER_API_URL ||
    ''
  ).replace(/\/$/, '');

  const walletKey =
    overrides.walletKey ||
    process.env.LLM_ROUTER_WALLET_KEY ||
    '';

  const defaultModel = overrides.defaultModel || 'auto';

  if (!gatewayUrl) {
    throw new ConfigError(
      'LLM_ROUTER_API_URL is required. Set it to your Solvela gateway URL.',
    );
  }

  if (!walletKey) {
    throw new ConfigError(
      'LLM_ROUTER_WALLET_KEY is required. Set it to your base58-encoded Solana private key.',
    );
  }

  return { gatewayUrl, walletKey, defaultModel };
}
