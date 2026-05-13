export interface ClientConfig {
  gatewayUrl: string;
  rpcUrl: string;
  preferEscrow: boolean;
  timeout: number;
  expectedRecipient?: string;
  maxPaymentAmount?: number;
  enableCache: boolean;
  enableSessions: boolean;
  sessionTtl: number;
  enableQualityCheck: boolean;
  maxQualityRetries: number;
  freeFallbackModel?: string;
}

/**
 * Default cap on a single payment in USDC atomic units.
 * 10 USDC (10_000_000 atomic with 6 decimals). Acts as a guardrail so the
 * `maxPaymentAmount` cap is opt-out instead of opt-in.
 */
export const DEFAULT_MAX_PAYMENT_AMOUNT = 10_000_000;

export const DEFAULT_CONFIG: ClientConfig = {
  gatewayUrl: 'https://api.solvela.ai',
  rpcUrl: 'https://api.mainnet-beta.solana.com',
  preferEscrow: false,
  timeout: 180,
  maxPaymentAmount: DEFAULT_MAX_PAYMENT_AMOUNT,
  enableCache: false,
  enableSessions: false,
  sessionTtl: 1800,
  enableQualityCheck: false,
  maxQualityRetries: 1,
};

/**
 * Reject plaintext `http://` gateway URLs unless the host is a loopback
 * address. Prevents accidentally pointing the SDK at an unencrypted endpoint
 * in production (where credentials and signed payment payloads would be
 * exposed in transit).
 */
export function validateGatewayUrl(url: string): void {
  if (url.startsWith('http://')) {
    let parsed: URL;
    try {
      parsed = new URL(url);
    } catch {
      throw new Error(`Invalid gatewayUrl: ${String(url).slice(0, 200)}`);
    }
    // URL.hostname wraps IPv6 in brackets — strip them before comparison.
    const host = parsed.hostname.replace(/^\[/, '').replace(/\]$/, '');
    if (host !== 'localhost' && host !== '127.0.0.1' && host !== '::1') {
      throw new Error(
        'gatewayUrl must use https:// for non-local endpoints (got http://)',
      );
    }
  }
}

export class ClientBuilder {
  private config: ClientConfig = { ...DEFAULT_CONFIG };

  withGatewayUrl(url: string): ClientBuilder {
    validateGatewayUrl(url);
    return this.with({ gatewayUrl: url });
  }

  withRpcUrl(url: string): ClientBuilder {
    return this.with({ rpcUrl: url });
  }

  withPreferEscrow(prefer: boolean): ClientBuilder {
    return this.with({ preferEscrow: prefer });
  }

  withTimeout(seconds: number): ClientBuilder {
    return this.with({ timeout: seconds });
  }

  withExpectedRecipient(recipient: string): ClientBuilder {
    return this.with({ expectedRecipient: recipient });
  }

  withMaxPaymentAmount(amount: number): ClientBuilder {
    return this.with({ maxPaymentAmount: amount });
  }

  withEnableCache(enable: boolean): ClientBuilder {
    return this.with({ enableCache: enable });
  }

  withEnableSessions(enable: boolean): ClientBuilder {
    return this.with({ enableSessions: enable });
  }

  withSessionTtl(seconds: number): ClientBuilder {
    return this.with({ sessionTtl: seconds });
  }

  withEnableQualityCheck(enable: boolean): ClientBuilder {
    return this.with({ enableQualityCheck: enable });
  }

  withMaxQualityRetries(retries: number): ClientBuilder {
    return this.with({ maxQualityRetries: retries });
  }

  withFreeFallbackModel(model: string): ClientBuilder {
    return this.with({ freeFallbackModel: model });
  }

  build(): ClientConfig {
    return { ...this.config };
  }

  private with(partial: Partial<ClientConfig>): ClientBuilder {
    const next = new ClientBuilder();
    next.config = { ...this.config, ...partial };
    return next;
  }
}
