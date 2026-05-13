/**
 * F4 — post-stream escrow settle hook.
 *
 * Observes the inner AssistantMessageEventStream's `result()` Promise. On
 * stream completion, fires a fire-and-forget POST to the gateway's
 * `/v1/escrow/settle` endpoint with the actual token usage, so the gateway
 * can claim the correct escrow amount based on real usage rather than
 * relying on the timeout-based auto-claim fallback.
 *
 * Status: client-side complete; the gateway endpoint at `/v1/escrow/settle`
 * does not exist yet (planned as a separate PR). Until that lands, settle
 * POSTs return 404. Failures are logged to stderr but **never** crash the
 * stream consumer or block the response.
 *
 * Usage:
 *
 *     import { wrapStreamForF4 } from '@solvela/openclaw-provider';
 *
 *     const stream = inner(model, context, options);
 *     wrapStreamForF4(stream, {
 *       serviceId,
 *       agentPubkey,
 *       modelId: model.id,
 *       gatewayUrl: SOLVELA_API_URL,
 *     });
 *     return stream;
 *
 * Why side-effect-only (not piping):
 *   We don't import `@earendil-works/pi-ai/utils/event-stream` because
 *   pi-ai's package.json does not export that subpath. Instead we rely on
 *   the EventStream's `.result()` Promise, which resolves when the producer
 *   calls `end()` regardless of who consumes the AsyncIterator. This avoids
 *   a 2-consumer race against OpenClaw's stream consumer.
 */

import type { AssistantMessage } from '@earendil-works/pi-ai';

/**
 * Structural type for any stream with a `.result()` Promise that resolves
 * with the final AssistantMessage. Matches `EventStream<E, AssistantMessage>`
 * from @earendil-works/pi-ai/utils/event-stream.
 */
export interface ResultableStream {
  result(): Promise<AssistantMessage>;
}

/** Inputs F4 needs to settle a single escrow deposit against actual usage. */
export interface F4Context {
  /** Service ID identifying the escrow deposit PDA. Hex/base58/base64 — gateway-defined encoding. */
  serviceId: string;
  /** Base58-encoded agent (payer) pubkey. */
  agentPubkey: string;
  /** The model ID the call was billed against (e.g. "gpt-4o", "auto"). */
  modelId: string;
  /** Solvela gateway base URL (no trailing slash). */
  gatewayUrl: string;
  /** Optional fetch override for testing. Defaults to global fetch. */
  fetchFn?: typeof fetch;
  /** Optional timeout in ms for the settle POST (default 5000). */
  timeoutMs?: number;
}

/** Optional stats tracker — used by tests and ops to count settle outcomes. */
export interface F4Stats {
  fired: number;
  succeeded: number;
  failed: number;
}

/**
 * Schedule a fire-and-forget escrow settle POST when the inner stream
 * completes. Returns the input stream unchanged (no piping).
 *
 * Errors during settle are logged to stderr but do not propagate. The
 * inner stream consumer is never blocked or impacted by F4.
 */
export function wrapStreamForF4<T extends ResultableStream | Promise<ResultableStream>>(
  innerStream: T,
  f4: F4Context,
  stats?: F4Stats,
): T {
  void scheduleSettle(innerStream, f4, stats);
  return innerStream;
}

async function scheduleSettle(
  innerOrPromise: ResultableStream | Promise<ResultableStream>,
  f4: F4Context,
  stats: F4Stats | undefined,
): Promise<void> {
  let finalMessage: AssistantMessage;
  try {
    const stream = await Promise.resolve(innerOrPromise);
    finalMessage = await stream.result();
  } catch (err) {
    process.stderr.write(
      `[solvela-openclaw] F4: stream.result() rejected — skipping settle: ${
        err instanceof Error ? err.message : String(err)
      }\n`,
    );
    return;
  }

  const isError =
    finalMessage.stopReason === 'error' || finalMessage.stopReason === 'aborted';
  if (stats) stats.fired += 1;
  try {
    await postSettle(f4, finalMessage, isError);
    if (stats) stats.succeeded += 1;
  } catch (err) {
    if (stats) stats.failed += 1;
    process.stderr.write(
      `[solvela-openclaw] F4: settle POST failed (non-blocking): ${
        err instanceof Error ? err.message : String(err)
      }\n`,
    );
  }
}

interface SettleBody {
  service_id: string;
  agent_pubkey: string;
  model: string;
  status: 'completed' | 'error';
  actual_prompt_tokens?: number;
  actual_completion_tokens?: number;
}

async function postSettle(
  f4: F4Context,
  finalMessage: AssistantMessage,
  isError: boolean,
): Promise<void> {
  const body: SettleBody = {
    service_id: f4.serviceId,
    agent_pubkey: f4.agentPubkey,
    model: f4.modelId,
    status: isError ? 'error' : 'completed',
  };
  if (finalMessage.usage) {
    body.actual_prompt_tokens = finalMessage.usage.input;
    body.actual_completion_tokens = finalMessage.usage.output;
  }

  const timeoutMs = f4.timeoutMs ?? 5000;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);

  const fetchFn = f4.fetchFn ?? fetch;
  try {
    const resp = await fetchFn(`${f4.gatewayUrl}/v1/escrow/settle`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
      redirect: 'error',
      signal: controller.signal,
    });
    if (!resp.ok) {
      throw new Error(`gateway settle returned ${resp.status} ${resp.statusText}`);
    }
  } finally {
    clearTimeout(timer);
  }
}
