import { ClientError } from './errors.js';

// ── Role ──

export type Role = 'system' | 'user' | 'assistant' | 'tool' | 'developer';

// ── Tool types ──

export interface FunctionCall {
  name: string;
  arguments: string;
}

export interface ToolCall {
  id: string;
  type: 'function';
  function: FunctionCall;
}

export interface FunctionCallDelta {
  name?: string;
  arguments?: string;
}

export interface ToolCallDelta {
  index: number;
  id?: string;
  type?: 'function';
  function?: FunctionCallDelta;
}

export interface FunctionDefinitionInner {
  name: string;
  description?: string;
  parameters?: Record<string, unknown>;
}

export interface ToolDefinition {
  type: 'function';
  function: FunctionDefinitionInner;
}

// ── Helper: omit undefined keys ──

function omitUndefined(obj: Record<string, unknown>): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(obj)) {
    if (v !== undefined) {
      result[k] = v;
    }
  }
  return result;
}

// ── ChatMessage ──

export class ChatMessage {
  constructor(
    public readonly role: Role,
    public readonly content: string | null,
    public readonly name?: string,
    public readonly toolCalls?: ToolCall[],
    public readonly toolCallId?: string,
  ) {}

  toJSON(): Record<string, unknown> {
    return omitUndefined({
      role: this.role,
      content: this.content,
      name: this.name,
      tool_calls: this.toolCalls,
      tool_call_id: this.toolCallId,
    });
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): ChatMessage {
    return new ChatMessage(
      data.role,
      data.content ?? null,
      data.name,
      data.tool_calls,
      data.tool_call_id,
    );
  }
}

// ── ChatRequest ──

export class ChatRequest {
  constructor(
    public readonly model: string,
    public readonly messages: ChatMessage[],
    public readonly maxTokens?: number,
    public readonly temperature?: number,
    public readonly topP?: number,
    public readonly stream: boolean = false,
    public readonly tools?: ToolDefinition[],
    public readonly toolChoice?: string | Record<string, unknown>,
  ) {}

  toJSON(): Record<string, unknown> {
    return omitUndefined({
      model: this.model,
      messages: this.messages.map((m) => m.toJSON()),
      max_tokens: this.maxTokens,
      temperature: this.temperature,
      top_p: this.topP,
      stream: this.stream,
      tools: this.tools,
      tool_choice: this.toolChoice,
    });
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): ChatRequest {
    return new ChatRequest(
      data.model,
      (data.messages ?? []).map((m: unknown) => ChatMessage.fromJSON(m)),
      data.max_tokens,
      data.temperature,
      data.top_p,
      data.stream ?? false,
      data.tools,
      data.tool_choice,
    );
  }
}

// ── Usage ──

export class Usage {
  constructor(
    public readonly promptTokens: number,
    public readonly completionTokens: number,
    public readonly totalTokens: number,
  ) {}

  toJSON(): Record<string, unknown> {
    return {
      prompt_tokens: this.promptTokens,
      completion_tokens: this.completionTokens,
      total_tokens: this.totalTokens,
    };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): Usage {
    return new Usage(
      data.prompt_tokens,
      data.completion_tokens,
      data.total_tokens,
    );
  }
}

// ── ChatChoice ──

export class ChatChoice {
  constructor(
    public readonly index: number,
    public readonly message: ChatMessage,
    public readonly finishReason?: string | null,
  ) {}

  toJSON(): Record<string, unknown> {
    return omitUndefined({
      index: this.index,
      message: this.message.toJSON(),
      finish_reason: this.finishReason,
    });
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): ChatChoice {
    return new ChatChoice(
      data.index,
      ChatMessage.fromJSON(data.message),
      data.finish_reason,
    );
  }
}

// ── ChatResponse ──

export class ChatResponse {
  constructor(
    public readonly id: string,
    public readonly object: string,
    public readonly created: number,
    public readonly model: string,
    public readonly choices: ChatChoice[],
    public readonly usage?: Usage,
  ) {}

  toJSON(): Record<string, unknown> {
    return omitUndefined({
      id: this.id,
      object: this.object,
      created: this.created,
      model: this.model,
      choices: this.choices.map((c) => c.toJSON()),
      usage: this.usage?.toJSON(),
    });
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): ChatResponse {
    return new ChatResponse(
      data.id,
      data.object,
      data.created,
      data.model,
      (data.choices ?? []).map((c: unknown) => ChatChoice.fromJSON(c)),
      data.usage ? Usage.fromJSON(data.usage) : undefined,
    );
  }
}

// ── Streaming types ──

export class ChatDelta {
  constructor(
    public readonly role?: Role,
    public readonly content?: string | null,
    public readonly toolCalls?: ToolCallDelta[],
  ) {}

  toJSON(): Record<string, unknown> {
    return omitUndefined({
      role: this.role,
      content: this.content,
      tool_calls: this.toolCalls,
    });
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): ChatDelta {
    return new ChatDelta(data.role, data.content, data.tool_calls);
  }
}

export class ChatChunkChoice {
  constructor(
    public readonly index: number,
    public readonly delta: ChatDelta,
    public readonly finishReason: string | null,
  ) {}

  toJSON(): Record<string, unknown> {
    return {
      index: this.index,
      delta: this.delta.toJSON(),
      finish_reason: this.finishReason,
    };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): ChatChunkChoice {
    return new ChatChunkChoice(
      data.index,
      ChatDelta.fromJSON(data.delta),
      data.finish_reason ?? null,
    );
  }
}

export class ChatChunk {
  constructor(
    public readonly id: string,
    public readonly object: string,
    public readonly created: number,
    public readonly model: string,
    public readonly choices: ChatChunkChoice[],
  ) {}

  toJSON(): Record<string, unknown> {
    return {
      id: this.id,
      object: this.object,
      created: this.created,
      model: this.model,
      choices: this.choices.map((c) => c.toJSON()),
    };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): ChatChunk {
    return new ChatChunk(
      data.id,
      data.object,
      data.created,
      data.model,
      (data.choices ?? []).map((c: unknown) => ChatChunkChoice.fromJSON(c)),
    );
  }
}

// ── Payment types ──

/**
 * Known x402 payment schemes. A directly parsed PaymentAccept carrying an
 * unknown scheme indicates either a malformed payload or a protocol upgrade
 * the SDK has not been taught yet; either way, fail fast at parse time rather
 * than silently mis-branching at the scheme-matching call site. Mirrors
 * Python's `Scheme = Literal["exact", "escrow"]` + `_KNOWN_SCHEMES` guard.
 * A 402 body advertising a scheme not in this set still parses — the entry is
 * skipped-and-represented (see `PaymentRequired.fromJSON`), never selected.
 */
export type Scheme = 'exact' | 'escrow';

const KNOWN_SCHEMES: ReadonlySet<string> = new Set<Scheme>(['exact', 'escrow']);

function parseScheme(raw: unknown): Scheme {
  if (typeof raw !== 'string' || !KNOWN_SCHEMES.has(raw)) {
    throw new ClientError(`Unknown payment scheme: ${JSON.stringify(raw).slice(0, 64)}`);
  }
  return raw as Scheme;
}

export class Resource {
  constructor(
    public readonly url: string,
    public readonly method: string,
  ) {}

  toJSON(): Record<string, unknown> {
    return {
      url: this.url,
      method: this.method,
    };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): Resource {
    return new Resource(data.url, data.method);
  }
}

export class PaymentAccept {
  constructor(
    public readonly scheme: Scheme,
    public readonly network: string,
    public readonly amount: string,
    public readonly asset: string,
    public readonly payTo: string,
    public readonly maxTimeoutSeconds: number,
    public readonly escrowProgramId?: string,
  ) {}

  toJSON(): Record<string, unknown> {
    return omitUndefined({
      scheme: this.scheme,
      network: this.network,
      amount: this.amount,
      asset: this.asset,
      pay_to: this.payTo,
      max_timeout_seconds: this.maxTimeoutSeconds,
      escrow_program_id: this.escrowProgramId,
    });
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): PaymentAccept {
    return new PaymentAccept(
      parseScheme(data.scheme),
      data.network,
      data.amount,
      data.asset,
      data.pay_to,
      data.max_timeout_seconds,
      data.escrow_program_id,
    );
  }
}

export class CostBreakdown {
  constructor(
    public readonly providerCost: string,
    public readonly platformFee: string,
    public readonly total: string,
    public readonly currency: string,
    public readonly feePercent: number,
  ) {}

  toJSON(): Record<string, unknown> {
    return {
      provider_cost: this.providerCost,
      platform_fee: this.platformFee,
      total: this.total,
      currency: this.currency,
      fee_percent: this.feePercent,
    };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): CostBreakdown {
    return new CostBreakdown(
      data.provider_cost,
      data.platform_fee,
      data.total,
      data.currency,
      data.fee_percent,
    );
  }
}

/**
 * 402 Payment Required response body.
 *
 * `accepts` holds only the entries whose scheme this SDK implements; entries
 * with an unrecognized scheme (e.g. a future `channel` advert) are skipped at
 * parse time and represented in `unrecognizedSchemes` (SDK-side only — never
 * serialized back to the wire). Skipping is forward-compatibility, NOT
 * selection: scheme selection stays fail-closed and the client throws
 * PaymentRequiredError when no recognized entry remains.
 */
export class PaymentRequired {
  constructor(
    public readonly x402Version: number,
    public readonly resource: Resource,
    public readonly accepts: PaymentAccept[],
    public readonly costBreakdown: CostBreakdown,
    public readonly error: string,
    public readonly unrecognizedSchemes: readonly string[] = [],
  ) {}

  toJSON(): Record<string, unknown> {
    return {
      x402_version: this.x402Version,
      resource: this.resource.toJSON(),
      accepts: this.accepts.map((a) => a.toJSON()),
      cost_breakdown: this.costBreakdown.toJSON(),
      error: this.error,
    };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): PaymentRequired {
    const accepts: PaymentAccept[] = [];
    const unrecognized: string[] = [];
    for (const entry of data.accepts ?? []) {
      const scheme = (entry as { scheme?: unknown } | null)?.scheme;
      if (typeof scheme === 'string' && KNOWN_SCHEMES.has(scheme)) {
        // Known-scheme entries stay fully strict — a malformed exact/escrow
        // advert is still a loud parse failure, never signable.
        accepts.push(PaymentAccept.fromJSON(entry));
      } else {
        // Skip-or-represent, never select: an entry this SDK cannot
        // understand (unknown scheme, or no parseable scheme at all) must
        // not poison the whole 402 for the entries it CAN pay (Slice B of
        // the channel plan). Truncated — the value is untrusted gateway data.
        unrecognized.push(String(scheme).slice(0, 64));
      }
    }
    return new PaymentRequired(
      data.x402_version,
      Resource.fromJSON(data.resource),
      accepts,
      CostBreakdown.fromJSON(data.cost_breakdown),
      data.error,
      unrecognized,
    );
  }
}

export class SolanaPayload {
  constructor(public readonly transaction: string) {}

  toJSON(): Record<string, unknown> {
    return { transaction: this.transaction };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): SolanaPayload {
    return new SolanaPayload(data.transaction);
  }
}

export class EscrowPayload {
  constructor(
    public readonly depositTx: string,
    public readonly serviceId: string,
    public readonly agentPubkey: string,
  ) {}

  toJSON(): Record<string, unknown> {
    return {
      deposit_tx: this.depositTx,
      service_id: this.serviceId,
      agent_pubkey: this.agentPubkey,
    };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): EscrowPayload {
    return new EscrowPayload(
      data.deposit_tx,
      data.service_id,
      data.agent_pubkey,
    );
  }
}

export class PaymentPayload {
  constructor(
    public readonly x402Version: number,
    public readonly resource: Resource,
    public readonly accepted: PaymentAccept,
    public readonly payload: SolanaPayload | EscrowPayload,
  ) {}

  toJSON(): Record<string, unknown> {
    return {
      x402_version: this.x402Version,
      resource: this.resource.toJSON(),
      accepted: this.accepted.toJSON(),
      payload: this.payload.toJSON(),
    };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): PaymentPayload {
    const payloadData = data.payload;
    // Discriminate by presence of "transaction" (mirrors Python's check):
    // SolanaPayload has only {transaction}, EscrowPayload has {deposit_tx, ...}.
    const payload =
      'transaction' in payloadData
        ? SolanaPayload.fromJSON(payloadData)
        : EscrowPayload.fromJSON(payloadData);
    return new PaymentPayload(
      data.x402_version,
      Resource.fromJSON(data.resource),
      PaymentAccept.fromJSON(data.accepted),
      payload,
    );
  }
}

// ── ModelInfo ──

/**
 * Model metadata from the gateway's `GET /v1/models` registry.
 *
 * Mirrors Python's ModelInfo dataclass: the wire format nests capabilities
 * under `capabilities: {...}` and pricing under `pricing: {...}`, but the
 * class flattens them so internal code stays terse. Pricing values are
 * USDC per million tokens as floats (e.g. 0.28 for DeepSeek's input rate),
 * NOT atomic units — convert at the boundary if needed.
 */
export class ModelInfo {
  constructor(
    public readonly id: string,
    public readonly provider: string,
    public readonly displayName: string,
    public readonly contextWindow: number,
    public readonly supportsStreaming: boolean = false,
    public readonly supportsTools: boolean = false,
    public readonly supportsVision: boolean = false,
    public readonly reasoning: boolean = false,
    public readonly inputUsdcPerMillion: number = 0,
    public readonly outputUsdcPerMillion: number = 0,
    public readonly currency: string = 'USDC',
    public readonly feePercent: number = 5,
  ) {}

  toJSON(): Record<string, unknown> {
    return {
      id: this.id,
      object: 'model',
      provider: this.provider,
      display_name: this.displayName,
      context_window: this.contextWindow,
      capabilities: {
        streaming: this.supportsStreaming,
        tools: this.supportsTools,
        vision: this.supportsVision,
        reasoning: this.reasoning,
      },
      pricing: {
        input_per_million: this.inputUsdcPerMillion,
        output_per_million: this.outputUsdcPerMillion,
        currency: this.currency,
        fee_percent: this.feePercent,
      },
    };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static fromJSON(data: any): ModelInfo {
    const caps = data.capabilities ?? {};
    const pricing = data.pricing ?? {};
    return new ModelInfo(
      data.id,
      data.provider,
      data.display_name,
      Number(data.context_window),
      Boolean(caps.streaming),
      Boolean(caps.tools),
      Boolean(caps.vision),
      Boolean(caps.reasoning),
      Number(pricing.input_per_million ?? 0),
      Number(pricing.output_per_million ?? 0),
      pricing.currency ?? 'USDC',
      Number(pricing.fee_percent ?? 5),
    );
  }
}
