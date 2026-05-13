import { ChatChunk, ChatRequest, ChatResponse, PaymentRequired } from './types.js';
import { GatewayError, PaymentRequiredError, TimeoutError } from './errors.js';

/**
 * Strip control characters and truncate gateway-supplied error strings before
 * surfacing them through Error messages. Prevents log injection / terminal
 * escape attacks via attacker-controlled upstream error bodies.
 */
function sanitizeErrorMessage(s: string): string {
  // eslint-disable-next-line no-control-regex
  return String(s).replace(/[\r\n\x00-\x1f]/g, ' ').slice(0, 200);
}

/**
 * Unwrap the gateway's 402 envelope around a PaymentRequired payload.
 *
 * The gateway wraps 402 bodies in an OpenAI-style error envelope:
 *   {"error": {"message": "<stringified PaymentRequired JSON>",
 *              "type": "invalid_payment"}}
 *
 * Returns the inner parsed payload when the envelope is present; otherwise
 * returns the body untouched so the caller's PaymentRequired.fromJSON
 * surfaces a clear error if the wire shape changes upstream.
 */
function unwrapPaymentRequiredEnvelope(body: unknown): unknown {
  if (typeof body !== 'object' || body === null) return body;
  const outer = body as { error?: unknown };
  if (typeof outer.error !== 'object' || outer.error === null) return body;
  const inner = outer.error as { message?: unknown };
  if (typeof inner.message !== 'string') return body;
  try {
    return JSON.parse(inner.message);
  } catch {
    return body;
  }
}

export class Transport {
  constructor(
    private readonly baseUrl: string,
    private readonly timeout: number = 180,
  ) {}

  private buildUrl(path: string): string {
    return `${this.baseUrl.replace(/\/$/, '')}${path}`;
  }

  private buildHeaders(
    paymentSignature?: string,
    extraHeaders?: Record<string, string>,
  ): Record<string, string> {
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    if (paymentSignature) headers['Payment-Signature'] = paymentSignature;
    if (extraHeaders) Object.assign(headers, extraHeaders);
    return headers;
  }

  async sendChat(
    request: ChatRequest,
    paymentSignature?: string,
    extraHeaders?: Record<string, string>,
  ): Promise<ChatResponse | PaymentRequired> {
    const url = this.buildUrl('/v1/chat/completions');
    const headers = this.buildHeaders(paymentSignature, extraHeaders);
    const body = request.toJSON();
    body.stream = false;

    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), this.timeout * 1000);

    try {
      const resp = await fetch(url, {
        method: 'POST',
        headers,
        body: JSON.stringify(body),
        signal: controller.signal,
      });
      clearTimeout(timeoutId);

      if (resp.status === 200) {
        return ChatResponse.fromJSON(await resp.json());
      } else if (resp.status === 402) {
        return PaymentRequired.fromJSON(unwrapPaymentRequiredEnvelope(await resp.json()));
      } else {
        const data = (await resp.json().catch(() => ({}))) as Record<string, unknown>;
        throw new GatewayError(
          resp.status,
          sanitizeErrorMessage((data.error as string) || resp.statusText),
        );
      }
    } catch (e) {
      clearTimeout(timeoutId);
      if (e instanceof GatewayError || e instanceof PaymentRequiredError) throw e;
      if ((e as Error).name === 'AbortError') throw new TimeoutError(this.timeout);
      throw e;
    }
  }

  async *sendChatStream(
    request: ChatRequest,
    paymentSignature?: string,
    extraHeaders?: Record<string, string>,
  ): AsyncGenerator<ChatChunk> {
    const url = this.buildUrl('/v1/chat/completions');
    const headers = this.buildHeaders(paymentSignature, extraHeaders);
    const body = request.toJSON();
    body.stream = true;

    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), this.timeout * 1000);

    let resp: Response;
    try {
      resp = await fetch(url, {
        method: 'POST',
        headers,
        body: JSON.stringify(body),
        signal: controller.signal,
      });
    } catch (e) {
      clearTimeout(timeoutId);
      if ((e as Error).name === 'AbortError') throw new TimeoutError(this.timeout);
      throw e;
    }
    clearTimeout(timeoutId);

    if (resp.status === 402) {
      const pr = PaymentRequired.fromJSON(unwrapPaymentRequiredEnvelope(await resp.json()));
      throw new PaymentRequiredError(pr);
    }
    if (resp.status !== 200) {
      const data = (await resp.json().catch(() => ({}))) as Record<string, unknown>;
      throw new GatewayError(
        resp.status,
        sanitizeErrorMessage((data.error as string) || resp.statusText),
      );
    }

    if (!resp.body) {
      throw new GatewayError(200, 'Response body is null');
    }
    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop() || '';
      for (const line of lines) {
        if (line.startsWith('data: ')) {
          const dataStr = line.slice(6).trim();
          if (dataStr === '[DONE]') return;
          let parsed: unknown;
          try {
            parsed = JSON.parse(dataStr);
          } catch (err) {
            // Malformed SSE chunk from gateway — skip rather than crash the
            // entire stream. Truncate the offending payload before logging
            // so an attacker-controlled body cannot pollute logs.
            // eslint-disable-next-line no-console
            console.warn(
              '[solvela] malformed SSE chunk, skipping:',
              sanitizeErrorMessage(dataStr),
              (err as Error).message,
            );
            continue;
          }
          yield ChatChunk.fromJSON(parsed);
        }
      }
    }
  }

  async fetchModels(): Promise<Record<string, unknown>[]> {
    const url = this.buildUrl('/v1/models');
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), this.timeout * 1000);
    try {
      const resp = await fetch(url, { signal: controller.signal });
      clearTimeout(timeoutId);
      if (resp.status !== 200) {
        throw new GatewayError(resp.status, resp.statusText);
      }
      const data = (await resp.json()) as { data?: Record<string, unknown>[] };
      return data.data ?? [];
    } catch (e) {
      clearTimeout(timeoutId);
      if (e instanceof GatewayError) throw e;
      if ((e as Error).name === 'AbortError') throw new TimeoutError(this.timeout);
      throw e;
    }
  }
}
