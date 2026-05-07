#!/usr/bin/env node
/**
 * Solvela MCP Server
 *
 * Exposes Solvela gateway capabilities as MCP tools so that
 * Claude Code, OpenClaw agents, and any MCP-compatible host can pay for
 * LLM calls with USDC on Solana transparently.
 *
 * Usage:
 *   npx @solvela/mcp-server
 *
 * Environment variables:
 *   SOLVELA_API_URL             Gateway URL (default: https://api.solvela.ai)
 *   SOLVELA_SESSION_BUDGET      Max USDC to spend this session (e.g. "1.00")
 *   SOLVELA_TIMEOUT_MS          Request timeout in ms (default: 60000)
 *   SOLVELA_SIGNING_MODE        auto | escrow | direct | off (default: auto)
 *   SOLVELA_ALLOW_DEV_BYPASS    Set to "1" to silence dev_bypass_payment warning
 *   SOLVELA_ESCROW_MODE         Set to "enabled" to expose the deposit_escrow tool
 *   SOLVELA_MAX_ESCROW_DEPOSIT  Per-call deposit cap in USDC (default: 5.0)
 *   SOLVELA_MAX_ESCROW_SESSION  Cumulative session deposit cap in USDC (default: 20.0)
 *   SOLANA_WALLET_KEY           Base58 secret key (required unless SOLVELA_SIGNING_MODE=off)
 *   SOLANA_RPC_URL              Solana RPC endpoint (required unless SOLVELA_SIGNING_MODE=off)
 *   SOLANA_WALLET_ADDRESS       Wallet pubkey shown in wallet_status / spending
 */

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  ErrorCode,
  McpError,
} from '@modelcontextprotocol/sdk/types.js';

import { GatewayClient, type ChatMessage } from './client.js';
import { getTools } from './tools.js';
import { createSessionStore } from './session.js';
import { createPaymentHeader, decodePaymentHeader, isStubTransaction } from '@solvela/signer-core';
import type { PaymentRequired, PaymentAccept } from '@solvela/signer-core';
import { Connection, PublicKey } from '@solana/web3.js';

// ---------------------------------------------------------------------------
// Bootstrap client from environment
// ---------------------------------------------------------------------------

// HF10: Validate SOLVELA_SIGNING_MODE before use.
const rawSigningMode = process.env['SOLVELA_SIGNING_MODE'] ?? 'auto';
if (!['auto', 'escrow', 'direct', 'off'].includes(rawSigningMode)) {
  process.stderr.write(
    `[solvela-mcp] Fatal: invalid SOLVELA_SIGNING_MODE='${rawSigningMode}'. Must be one of auto|escrow|direct|off.\n`,
  );
  process.exit(1);
}
const signingMode = rawSigningMode as 'auto' | 'escrow' | 'direct' | 'off';

// HF11: Validate SOLVELA_SESSION_BUDGET — reject NaN/non-positive values.
const budgetStr = process.env['SOLVELA_SESSION_BUDGET'] ?? process.env['RCR_SESSION_BUDGET']; // compat
let sessionBudget: number | undefined;
if (budgetStr !== undefined) {
  const parsed = parseFloat(budgetStr);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    process.stderr.write(
      `[solvela-mcp] Fatal: SOLVELA_SESSION_BUDGET='${budgetStr}' is not a positive number.\n`,
    );
    process.exit(1);
  }
  sessionBudget = parsed;
}

// HF11: Validate SOLVELA_TIMEOUT_MS — reject NaN/non-positive values.
const timeoutStr = process.env['SOLVELA_TIMEOUT_MS'] ?? process.env['RCR_TIMEOUT_MS']; // compat
let timeoutMs: number | undefined;
if (timeoutStr !== undefined) {
  const parsed = parseInt(timeoutStr, 10);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    process.stderr.write(
      `[solvela-mcp] Fatal: SOLVELA_TIMEOUT_MS='${timeoutStr}' is not a positive integer.\n`,
    );
    process.exit(1);
  }
  timeoutMs = parsed;
}

// T-2G-D: Validate SOLVELA_ESCROW_MODE — accept only 'enabled' or unset.
const rawEscrowMode = process.env['SOLVELA_ESCROW_MODE'];
if (rawEscrowMode !== undefined && rawEscrowMode !== 'enabled') {
  process.stderr.write(
    `[solvela-mcp] Fatal: invalid SOLVELA_ESCROW_MODE='${rawEscrowMode}'. ` +
    `Must be 'enabled' or unset. Set SOLVELA_ESCROW_MODE=enabled to activate escrow tools.\n`,
  );
  process.exit(1);
}
const escrowEnabled = rawEscrowMode === 'enabled';

// When escrow is enabled, SOLVELA_ESCROW_PROGRAM_ID and SOLVELA_RECIPIENT_WALLET
// are required — neither has a safe default. Validation happens in main() below.
const escrowProgramId = process.env['SOLVELA_ESCROW_PROGRAM_ID'] ?? '';
const escrowRecipientWallet = process.env['SOLVELA_RECIPIENT_WALLET'] ?? '';

// T-2G-D: Validate SOLVELA_MAX_ESCROW_DEPOSIT (default $5).
const maxEscrowDepositStr = process.env['SOLVELA_MAX_ESCROW_DEPOSIT'];
let maxEscrowDeposit = 5.0;
if (maxEscrowDepositStr !== undefined) {
  const parsed = parseFloat(maxEscrowDepositStr);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    process.stderr.write(
      `[solvela-mcp] Fatal: SOLVELA_MAX_ESCROW_DEPOSIT='${maxEscrowDepositStr}' is not a positive number.\n`,
    );
    process.exit(1);
  }
  maxEscrowDeposit = parsed;
}

// T-2G-D: Validate SOLVELA_MAX_ESCROW_SESSION (default $20).
const maxEscrowSessionStr = process.env['SOLVELA_MAX_ESCROW_SESSION'];
let maxEscrowSession = 20.0;
if (maxEscrowSessionStr !== undefined) {
  const parsed = parseFloat(maxEscrowSessionStr);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    process.stderr.write(
      `[solvela-mcp] Fatal: SOLVELA_MAX_ESCROW_SESSION='${maxEscrowSessionStr}' is not a positive number.\n`,
    );
    process.exit(1);
  }
  maxEscrowSession = parsed;
}

// T-1K-B: Wire session store for persistence.
const sessionStore = createSessionStore();

const client = new GatewayClient({
  apiUrl: process.env['SOLVELA_API_URL'] ?? process.env['RCR_API_URL'], // compat
  sessionBudget,
  timeoutMs,
  signingMode,
  sessionStore,
});

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

const server = new Server(
  {
    name: 'solvela',
    version: '0.1.0',
  },
  {
    capabilities: { tools: {} },
  },
);

// ---- list tools -----------------------------------------------------------

server.setRequestHandler(ListToolsRequestSchema, async () => ({
  // H5: Pass escrowEnabled as argument so getTools() doesn't re-read process.env per call.
  tools: getTools({ escrowEnabled }),
}));

// ---- call tool ------------------------------------------------------------

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;

  try {
    switch (name) {
      case 'chat': {
        const { model, prompt, system, max_tokens, temperature } = args as {
          model: string;
          prompt: string;
          system?: string;
          max_tokens?: number;
          temperature?: number;
        };

        const messages: ChatMessage[] = [];
        if (system) messages.push({ role: 'system', content: system });
        messages.push({ role: 'user', content: prompt });

        const response = await client.chat(model, messages, { maxTokens: max_tokens, temperature });
        const reply = response.choices[0]?.message.content ?? '';

        return {
          content: [
            {
              type: 'text',
              text: reply,
            },
            {
              type: 'text',
              text: formatUsage(response),
            },
          ],
        };
      }

      case 'smart_chat': {
        const { prompt, profile = 'auto', system, max_tokens } = args as {
          prompt: string;
          profile?: string;
          system?: string;
          max_tokens?: number;
        };

        const messages: ChatMessage[] = [];
        if (system) messages.push({ role: 'system', content: system });
        messages.push({ role: 'user', content: prompt });

        const response = await client.chat(profile, messages, { maxTokens: max_tokens });
        const reply = response.choices[0]?.message.content ?? '';

        return {
          content: [
            { type: 'text', text: reply },
            { type: 'text', text: formatUsage(response) },
          ],
        };
      }

      case 'wallet_status': {
        const health = await client.health();
        const walletAddress = process.env['SOLANA_WALLET_ADDRESS'] ?? 'not configured';
        const spend = client.spendSummary();

        const lines = [
          `Gateway:        ${client.apiUrl}`,
          `Status:         ${health.status}`,
          health.solana_rpc ? `Solana RPC:     ${health.solana_rpc}` : null,
          `Wallet:         ${walletAddress}`,
          `Session spent:  ${spend.session_usdc_spent} USDC`,
          spend.budget_remaining !== null
            ? `Budget left:    ${spend.budget_remaining} USDC`
            : null,
        ]
          .filter(Boolean)
          .join('\n');

        return { content: [{ type: 'text', text: lines }] };
      }

      case 'list_models': {
        const { filter } = args as { filter?: string };
        const modelsResp = await client.listModels();

        let models = modelsResp.data;
        if (filter) {
          const lower = filter.toLowerCase();
          models = models.filter((m) => m.id.toLowerCase().includes(lower));
        }

        if (models.length === 0) {
          return {
            content: [{ type: 'text', text: `No models found matching "${filter}".` }],
          };
        }

        const rows = models.map((m) => {
          const inputPrice = m.usdc_price_per_million_input
            ? `$${m.usdc_price_per_million_input}/M in`
            : '';
          const outputPrice = m.usdc_price_per_million_output
            ? `$${m.usdc_price_per_million_output}/M out`
            : '';
          const pricing = [inputPrice, outputPrice].filter(Boolean).join(', ');
          return `  ${m.id.padEnd(45)} ${pricing || '(see gateway)'}`;
        });

        const text = [`Available models (${models.length}):`, ...rows].join('\n');
        return { content: [{ type: 'text', text }] };
      }

      case 'spending': {
        const { reset = false } = (args ?? {}) as { reset?: boolean };

        if (reset) {
          await client.resetSession();
          return {
            content: [
              {
                type: 'text',
                text: 'Session counters reset. ~/.solvela/mcp-session.json cleared.',
              },
            ],
          };
        }

        const spend = client.spendSummary();
        const escrowTotal = client.getEscrowDepositsSession();

        const lines = [
          `Wallet:          ${spend.wallet_address ?? 'not configured'}`,
          `Requests:        ${spend.total_requests}`,
          `Session spent:   ${spend.session_usdc_spent} USDC`,
          ...(escrowEnabled
            ? [`Escrow deposits: ${escrowTotal.toFixed(6)} USDC (session cap: $${maxEscrowSession.toFixed(2)})`]
            : []),
          spend.budget_remaining !== null
            ? `Budget remaining: ${spend.budget_remaining} USDC`
            : 'Budget:          unlimited',
        ];

        return { content: [{ type: 'text', text: lines.join('\n') }] };
      }

      case 'deposit_escrow': {
        // T-2G-C: deposit_escrow handler
        if (!escrowEnabled) {
          throw new McpError(
            ErrorCode.MethodNotFound,
            'deposit_escrow is disabled. Set SOLVELA_ESCROW_MODE=enabled to enable.',
          );
        }

        // deposit_escrow always requires real signing — SOLANA_WALLET_KEY and SOLANA_RPC_URL
        // must be present regardless of SOLVELA_SIGNING_MODE.
        const privateKey = process.env['SOLANA_WALLET_KEY'];
        if (!privateKey) {
          throw new McpError(
            ErrorCode.InvalidRequest,
            'deposit_escrow requires SOLANA_WALLET_KEY to be set.',
          );
        }
        const rpcUrl = process.env['SOLANA_RPC_URL'];
        if (!rpcUrl) {
          throw new McpError(
            ErrorCode.InvalidRequest,
            'deposit_escrow requires SOLANA_RPC_URL to be set.',
          );
        }

        const { amount_usdc, max_timeout_seconds = 300 } = args as {
          amount_usdc: string;
          max_timeout_seconds?: number;
        };

        // H3: Strict amount parsing — reject scientific notation, trailing garbage, non-positive.
        // Regex accepts only plain decimal strings: digits optionally followed by '.' + digits.
        if (!/^\d+(\.\d+)?$/.test(amount_usdc)) {
          throw new McpError(
            ErrorCode.InvalidParams,
            `amount_usdc must be a positive decimal number (e.g. "5.00"), got: ${JSON.stringify(amount_usdc)}`,
          );
        }
        const amount = Number(amount_usdc);
        if (!Number.isFinite(amount) || amount <= 0) {
          throw new McpError(
            ErrorCode.InvalidParams,
            `amount_usdc must be positive, got: ${amount_usdc}`,
          );
        }

        // Per-call cap
        if (amount > maxEscrowDeposit) {
          throw new McpError(
            ErrorCode.InvalidParams,
            `Deposit amount $${amount.toFixed(6)} USDC exceeds per-call cap $${maxEscrowDeposit.toFixed(2)} USDC ` +
              `(SOLVELA_MAX_ESCROW_DEPOSIT). Reduce the amount or raise the cap.`,
          );
        }

        // Convert USDC to micro-USDC (6 decimals) for the on-chain amount
        const amountMicroUsdc = Math.round(amount * 1_000_000);

        // Build a synthetic PaymentRequired with an escrow accept so we can reuse
        // createPaymentHeader from @solvela/sdk (buildEscrowDeposit is internal).
        // escrowProgramId and escrowRecipientWallet are validated at startup — never empty here.
        const syntheticPaymentRequired: PaymentRequired = {
          x402_version: 2,
          accepts: [
            {
              scheme: 'escrow',
              network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
              amount: String(amountMicroUsdc),
              asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
              pay_to: escrowRecipientWallet,
              max_timeout_seconds,
              escrow_program_id: escrowProgramId,
            } as PaymentAccept,
          ],
          cost_breakdown: {
            provider_cost: String(amount),
            platform_fee: '0',
            total: String(amount),
            currency: 'USDC',
            fee_percent: 0,
          },
          error: 'Escrow deposit',
        };

        let depositTxSignature = '';
        let escrowPda = '';
        // count-on-broadcast policy: confirmed=true unless confirmation timed out or failed.
        let confirmed = true;

        // Use runEscrowDeposit for atomic session-cap check + commit.
        // onDeposit MUST throw ONLY if broadcast itself failed — confirmation timeout is NOT
        // a throw; it sets confirmed=false so the reservation is NOT rolled back.
        const newTotal = await client.runEscrowDeposit(amount, maxEscrowSession, async () => {
          // Build the payment header via the SDK (this builds + signs the deposit tx)
          let paymentHeader: string;
          try {
            paymentHeader = await createPaymentHeader(
              syntheticPaymentRequired,
              `${client.apiUrl}/v1/escrow/deposit`,
              privateKey,
              '', // no request body for standalone deposit
            );
          } catch (err) {
            throw new Error(
              `Failed to build escrow deposit transaction: ${err instanceof Error ? err.message : String(err)}`,
            );
          }

          // Decode the header to extract the deposit_tx and service_id
          let decoded: {
            payload?: { deposit_tx?: string; service_id?: string; agent_pubkey?: string };
          };
          try {
            decoded = decodePaymentHeader(paymentHeader) as typeof decoded;
          } catch (err) {
            throw new Error(`Failed to decode payment header: ${err instanceof Error ? err.message : String(err)}`);
          }

          const depositTxB64 = decoded?.payload?.deposit_tx;
          const agentPubkey = decoded?.payload?.agent_pubkey;
          const serviceIdB64 = decoded?.payload?.service_id;

          if (!depositTxB64 || isStubTransaction(depositTxB64)) {
            throw new Error(
              'Escrow deposit tx is a stub — ensure SOLANA_WALLET_KEY is set and valid.',
            );
          }

          const connection = new Connection(rpcUrl, 'confirmed');

          // Fetch blockhash BEFORE broadcast so lastValidBlockHeight is available for confirmTransaction.
          const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash('confirmed') as {
            blockhash: string;
            lastValidBlockHeight: number;
          };

          const txBytes = Buffer.from(depositTxB64, 'base64');

          // Phase 2 boundary: ONLY throw if broadcast fails.
          // A broadcast failure means the tx was never submitted — safe to roll back the cap.
          let signature: string;
          try {
            signature = (await connection.sendRawTransaction(txBytes, {
              skipPreflight: false,
              preflightCommitment: 'confirmed',
            })) as string;
          } catch (err) {
            throw new Error(
              `Failed to broadcast escrow deposit: ${err instanceof Error ? err.message : String(err)}`,
            );
          }

          depositTxSignature = signature;

          // Confirmation: NOT part of the throw boundary.
          // If confirmation times out, the tx is in flight — count-on-broadcast means
          // the reservation stands. We signal pending via the confirmed flag.
          try {
            const confirmation = await connection.confirmTransaction(
              { signature, blockhash, lastValidBlockHeight },
              'confirmed',
            ) as { value: { err: unknown } };
            if (confirmation.value.err) {
              // Tx landed but failed on-chain — rare. Count as broadcast-succeeded.
              confirmed = false;
              process.stderr.write(
                `[solvela-mcp] WARN: deposit ${signature} landed but failed on-chain. ` +
                `Counted against session cap per count-on-broadcast policy.\n`,
              );
            }
          } catch (err) {
            // Confirmation timeout — tx is in flight, count it.
            confirmed = false;
            process.stderr.write(
              `[solvela-mcp] WARN: deposit ${signature} not confirmed within timeout; ` +
              `counted against session cap per count-on-broadcast policy. ` +
              `Check the signature on Solana Explorer.\n`,
            );
          }

          // Derive the escrow PDA to return to the caller.
          // Seeds: ["escrow", agentPubkey, serviceId]
          if (agentPubkey && serviceIdB64) {
            try {
              const [pda] = PublicKey.findProgramAddressSync(
                [
                  Buffer.from('escrow'),
                  new PublicKey(agentPubkey).toBuffer(),
                  Buffer.from(serviceIdB64, 'base64'),
                ],
                new PublicKey(escrowProgramId),
              );
              escrowPda = pda.toBase58();
            } catch {
              escrowPda = '(derivation failed — see agent_pubkey + service_id)';
            }
          }
        });

        return {
          content: [
            {
              type: 'text',
              text: JSON.stringify(
                {
                  deposit_tx_signature: depositTxSignature,
                  escrow_pda: escrowPda,
                  amount_deposited_usdc: amount.toFixed(6),
                  session_deposits_total_usdc: newTotal.toFixed(6),
                  session_deposits_cap_usdc: maxEscrowSession.toFixed(6),
                  confirmation_status: confirmed ? 'confirmed' : 'pending',
                },
                null,
                2,
              ),
            },
          ],
        };
      }

      default:
        throw new McpError(ErrorCode.MethodNotFound, `Unknown tool: ${name}`);
    }
  } catch (err) {
    if (err instanceof McpError) throw err;

    const message = err instanceof Error ? err.message : String(err);
    throw new McpError(ErrorCode.InternalError, message);
  }
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatUsage(response: { model: string; usage?: { prompt_tokens: number; completion_tokens: number; total_tokens: number } }): string {
  const u = response.usage;
  if (!u) return `Model: ${response.model}`;
  return (
    `Model: ${response.model} | ` +
    `Tokens: ${u.prompt_tokens} in / ${u.completion_tokens} out / ${u.total_tokens} total`
  );
}

// ---------------------------------------------------------------------------
// Start server
// ---------------------------------------------------------------------------

async function main() {
  // T1-D / T1-E: Startup validation — fail fast when signing is enabled without required keys.
  if (signingMode !== 'off') {
    if (!process.env['SOLANA_WALLET_KEY']) {
      process.stderr.write(
        'Fatal: SOLANA_WALLET_KEY is required when signing is enabled. Set SOLVELA_SIGNING_MODE=off to run without signing.\n',
      );
      process.exit(1);
    }
    if (!process.env['SOLANA_RPC_URL']) {
      process.stderr.write(
        'Fatal: SOLANA_RPC_URL is required when signing is enabled. Set SOLVELA_SIGNING_MODE=off to run without signing.\n',
      );
      process.exit(1);
    }
  }

  // T-2G + HF-F2T2-H4: Startup validation — escrow mode requires explicit program ID and
  // recipient wallet, both of which must be valid Solana base58 pubkeys.
  if (escrowEnabled) {
    if (!escrowProgramId) {
      process.stderr.write(
        'Fatal: SOLVELA_ESCROW_PROGRAM_ID required when SOLVELA_ESCROW_MODE=enabled.\n',
      );
      process.exit(1);
    }
    if (!escrowRecipientWallet) {
      process.stderr.write(
        'Fatal: SOLVELA_RECIPIENT_WALLET required when SOLVELA_ESCROW_MODE=enabled.\n',
      );
      process.exit(1);
    }
    // Validate format — rejects malformed or typo'd addresses early before any deposit.
    try {
      new PublicKey(escrowProgramId);
    } catch {
      process.stderr.write(
        `Fatal: SOLVELA_ESCROW_PROGRAM_ID=${escrowProgramId} is not a valid Solana pubkey.\n`,
      );
      process.exit(1);
    }
    try {
      new PublicKey(escrowRecipientWallet);
    } catch {
      process.stderr.write(
        `Fatal: SOLVELA_RECIPIENT_WALLET=${escrowRecipientWallet} is not a valid Solana pubkey.\n`,
      );
      process.exit(1);
    }
    // SOLANA_WALLET_KEY and SOLANA_RPC_URL are also required when escrow is enabled,
    // even if SOLVELA_SIGNING_MODE=off (escrow always needs real signing).
    if (!process.env['SOLANA_WALLET_KEY']) {
      process.stderr.write(
        'Fatal: SOLANA_WALLET_KEY required when SOLVELA_ESCROW_MODE=enabled.\n',
      );
      process.exit(1);
    }
    if (!process.env['SOLANA_RPC_URL']) {
      process.stderr.write(
        'Fatal: SOLANA_RPC_URL required when SOLVELA_ESCROW_MODE=enabled.\n',
      );
      process.exit(1);
    }
  }

  // HF6: Log resolved gateway URL and signing mode at startup — makes typos visible.
  process.stderr.write(
    `[solvela-mcp] gateway=${client.apiUrl} signingMode=${signingMode}\n`,
  );

  // T-2G-D: Log escrow mode and caps.
  process.stderr.write(
    `[solvela-mcp] escrow=${escrowEnabled ? 'enabled' : 'disabled'} max-deposit=$${maxEscrowDeposit.toFixed(2)} max-session=$${maxEscrowSession.toFixed(2)}\n`,
  );

  // HF7: Health check with short timeout (5 s) so it never blocks MCP handshake.
  const healthTimeoutMs = 5000;
  try {
    const healthController = new AbortController();
    const healthTimer = setTimeout(() => healthController.abort(), healthTimeoutMs);
    let health: Record<string, unknown>;
    try {
      const healthResp = await Promise.race([
        client.health(),
        new Promise<never>((_, reject) =>
          setTimeout(() => reject(new Error(`Health check timed out after ${healthTimeoutMs}ms`)), healthTimeoutMs),
        ),
      ]);
      health = healthResp as Record<string, unknown>;
    } finally {
      clearTimeout(healthTimer);
    }

    // HF12: Stronger warning when signing is ON but dev_bypass is active —
    // key is in memory but payments are not verified.
    if (health['dev_bypass_payment'] === true && process.env['SOLVELA_ALLOW_DEV_BYPASS'] !== '1') {
      if (signingMode !== 'off') {
        process.stderr.write(
          `[solvela-mcp] WARN: Gateway is in dev_bypass_payment mode but signingMode=${signingMode} — ` +
          `your wallet key is loaded in memory but the gateway is NOT verifying payments. ` +
          `Consider SOLVELA_SIGNING_MODE=off for dev. Set SOLVELA_ALLOW_DEV_BYPASS=1 to silence.\n`,
        );
      } else {
        process.stderr.write(
          '[solvela-mcp] WARN: Gateway is running in dev_bypass_payment mode. Payments will NOT be verified.' +
          ' Set SOLVELA_ALLOW_DEV_BYPASS=1 to silence this warning.\n',
        );
      }
    }
  } catch (err) {
    // HF7: Gateway unreachable or timed out — warn but do not prevent server from starting.
    const msg = err instanceof Error ? err.message : String(err);
    process.stderr.write(
      `[solvela-mcp] WARN: Gateway health check failed (${msg}). Gateway URL: ${client.apiUrl}. The server will start but chat calls may fail.\n`,
    );
  }

  const transport = new StdioServerTransport();
  await server.connect(transport);
  // Server runs until the host closes the connection
}

main().catch((err) => {
  process.stderr.write(`Fatal: ${err instanceof Error ? err.message : String(err)}\n`);
  process.exit(1);
});
