# Vercel AI SDK Custom Provider — Research Report

**Target:** Building a first-party Solvela provider for the Vercel AI SDK (`ai` package) so Next.js/TS agent developers can swap Solvela in as their LLM provider.
**Date:** 2026-04-16
**Stage:** Research (pre-plan). Stage 1 agent: `oh-my-claudecode:document-specialist`. Stage 2 (persist): `oh-my-claudecode:executor`.
**Next step:** Plan authoring, then plan review before any implementation.

---

## Research: Vercel AI SDK Custom Provider Specification for Solvela

### Executive Summary

The Vercel AI SDK (`ai` package, currently v6.x stable / v7 pre-release beta) uses a versioned provider interface system. As of April 2026, the current stable interface for production use is `LanguageModelV3` (shipped with `@ai-sdk/provider` 3.x, `ai` 6.x). A newer `LanguageModelV4` is in active pre-release (`@ai-sdk/provider` 4.0.0-beta.12, `ai` 7 pre-release), with `LanguageModelV3` and `LanguageModelV4` exported side-by-side. All packages are now **ESM-only** as of recent betas. The `@ai-sdk/openai-compatible` package exists and is the correct foundation to extend for Solvela — it wraps any OpenAI-compatible HTTP endpoint and exposes a `fetch` injection point and a `transformRequestBody` hook. The critical finding for the 402/sign/retry flow is that the SDK's `postToApi` utility routes **any non-2xx response** through a `failedResponseHandler` which throws an `APICallError`; there is no built-in retry on 402 (only on 408/409/429/5xx). The correct interception seam is either a **custom `fetch` function** injected at provider construction or a **`LanguageModelV4Middleware`** wrapping the model, both of which are clean, supported extension points.

---

## Section 1: Current Provider Specification

### Package Versions (authoritative as of 2026-04-16)

| Package | Current Stable | Pre-release (v7 beta) |
|---|---|---|
| `ai` | 6.0.162 | 7.x pre-release |
| `@ai-sdk/provider` | 3.x (stable) | 4.0.0-beta.12 |
| `@ai-sdk/provider-utils` | ~4.x (stable) | 5.0.0-beta.21 |
| `@ai-sdk/openai-compatible` | ~2.x (stable) | 3.0.0-beta.26 |

Sources:
- `ai` package npm: [https://www.npmjs.com/package/ai](https://www.npmjs.com/package/ai) (v6.0.162 latest stable)
- `@ai-sdk/provider` package.json in monorepo: `packages/provider/package.json` (`version: "4.0.0-beta.12"` on main branch)
- `@ai-sdk/openai-compatible` package.json: `packages/openai-compatible/package.json` (`version: "3.0.0-beta.26"` on main)

### Which Interface to Implement

The monorepo exports **three** versions from `packages/provider/src/language-model/index.ts`:

```typescript
export * from './v4/index';
export * from './v3/index';
export * from './v2/index';
```

Source: `packages/provider/src/language-model/index.ts` in [github.com/vercel/ai](https://github.com/vercel/ai/blob/main/packages/provider/src/language-model/index.ts)

**Target for new provider work: `LanguageModelV4`** (most current, on main branch / v7 beta). `LanguageModelV3` is the current stable-release interface used by `ai` v6. Since the v4 and v3 interfaces are structurally nearly identical (same required fields, same `doGenerate`/`doStream` signatures, same `supportedUrls` field), a provider implemented against V4 is trivially compatible with the `wrapLanguageModel` utility which accepts V2, V3, or V4 (see source below). **For immediate shipping against `ai` v6 stable, implement `LanguageModelV3` (`specificationVersion: 'v3'`). For targeting the v7 beta, implement `LanguageModelV4` (`specificationVersion: 'v4'`).**

The `OpenAICompatibleChatLanguageModel` in the monorepo already declares `specificationVersion = 'v4' as const` and implements `LanguageModelV4`.

Source: `packages/openai-compatible/src/chat/openai-compatible-chat-language-model.ts`, line ~72

### Required Interface Surface — `LanguageModelV4`

Source: `packages/provider/src/language-model/v4/language-model-v4.ts`

```typescript
export type LanguageModelV4 = {
  // REQUIRED metadata
  readonly specificationVersion: 'v4';
  readonly provider: string;        // e.g. "solvela.chat"
  readonly modelId: string;         // e.g. "claude-sonnet-4-5"

  // REQUIRED: URL patterns natively supported (no download needed)
  // Keys are IANA media type patterns; values are RegExp arrays matching URLs.
  supportedUrls:
    | PromiseLike<Record<string, RegExp[]>>
    | Record<string, RegExp[]>;

  // REQUIRED: non-streaming generation
  doGenerate(
    options: LanguageModelV4CallOptions,
  ): PromiseLike<LanguageModelV4GenerateResult>;

  // REQUIRED: streaming generation
  doStream(
    options: LanguageModelV4CallOptions,
  ): PromiseLike<LanguageModelV4StreamResult>;
};
```

There is **no** `defaultObjectGenerationMode` on the current V4 interface. That property existed in earlier V1 documentation but has been removed. There is also no `supportsImageUrls` — those were V1-era fields. The `supportedUrls` map is the current way to declare multimodal URL support.

**Notable difference from V3:** The interfaces are structurally identical between V3 and V4 — the only difference is the `specificationVersion` literal (`'v3'` vs `'v4'`).

### `LanguageModelV4CallOptions` — Full Field List

Source: `packages/provider/src/language-model/v4/language-model-v4-call-options.ts`

| Field | Type | Notes |
|---|---|---|
| `prompt` | `LanguageModelV4Prompt` | Required. Array of messages |
| `maxOutputTokens` | `number \| undefined` | |
| `temperature` | `number \| undefined` | |
| `stopSequences` | `string[] \| undefined` | |
| `topP` | `number \| undefined` | |
| `topK` | `number \| undefined` | |
| `presencePenalty` | `number \| undefined` | |
| `frequencyPenalty` | `number \| undefined` | |
| `responseFormat` | `{type:'text'} \| {type:'json', schema?, name?, description?}` | |
| `seed` | `number \| undefined` | |
| `tools` | `Array<LanguageModelV4FunctionTool \| LanguageModelV4ProviderTool>` | |
| `toolChoice` | `LanguageModelV4ToolChoice \| undefined` | |
| `includeRawChunks` | `boolean \| undefined` | For streaming |
| `abortSignal` | `AbortSignal \| undefined` | |
| `headers` | `Record<string, string \| undefined> \| undefined` | **Per-request headers — key for Solvela** |
| `reasoning` | `'provider-default' \| 'none' \| 'minimal' \| 'low' \| 'medium' \| 'high' \| 'xhigh' \| undefined` | |
| `providerOptions` | `SharedV4ProviderOptions \| undefined` | Provider-specific pass-through |

The `headers` field is the critical affordance — it allows callers to pass per-request headers that flow directly into the HTTP request. However, this is the *caller*-facing field, not the provider's internal header injection (see §4).

### `LanguageModelV4GenerateResult` — Return Shape from `doGenerate`

Source: `packages/provider/src/language-model/v4/language-model-v4-generate-result.ts`

```typescript
type LanguageModelV4GenerateResult = {
  content: Array<LanguageModelV4Content>;  // ordered array of output parts
  finishReason: LanguageModelV4FinishReason;
  usage: LanguageModelV4Usage;
  providerMetadata?: SharedV4ProviderMetadata;
  request?: { body?: unknown };            // optional, for telemetry
  response?: LanguageModelV4ResponseMetadata & {
    headers?: SharedV4Headers;
    body?: unknown;
  };
  warnings: Array<SharedV4Warning>;
};
```

`LanguageModelV4Content` is a union:
```typescript
type LanguageModelV4Content =
  | LanguageModelV4Text          // { type: 'text'; text: string; providerMetadata? }
  | LanguageModelV4Reasoning     // { type: 'reasoning'; text: string; ... }
  | LanguageModelV4CustomContent
  | LanguageModelV4ReasoningFile
  | LanguageModelV4File
  | LanguageModelV4ToolApprovalRequest
  | LanguageModelV4Source
  | LanguageModelV4ToolCall      // { type: 'tool-call'; toolCallId; toolName; input: string; providerExecuted?; dynamic?; providerMetadata? }
  | LanguageModelV4ToolResult;
```

`LanguageModelV4FinishReason` is now a **structured object** (changed from a plain string in V1/V2):
```typescript
type LanguageModelV4FinishReason = {
  unified: 'stop' | 'length' | 'content-filter' | 'tool-calls' | 'error' | 'other';
  raw: string | undefined;  // original string from provider
};
```

`LanguageModelV4Usage` — token tracking:
```typescript
type LanguageModelV4Usage = {
  inputTokens: {
    total: number | undefined;
    noCache: number | undefined;
    cacheRead: number | undefined;
    cacheWrite: number | undefined;
  };
  outputTokens: {
    total: number | undefined;
    text: number | undefined;
    reasoning: number | undefined;
  };
  raw?: JSONObject;
};
```

### `LanguageModelV4StreamResult` — Return Shape from `doStream`

Source: `packages/provider/src/language-model/v4/language-model-v4-stream-result.ts`

```typescript
type LanguageModelV4StreamResult = {
  stream: ReadableStream<LanguageModelV4StreamPart>;
  request?: { body?: unknown };
  response?: { headers?: SharedV4Headers };
};
```

### Stream Parts — Complete Union

Source: `packages/provider/src/language-model/v4/language-model-v4-stream-part.ts`

```typescript
type LanguageModelV4StreamPart =
  // Lifecycle — MUST emit stream-start first, finish last
  | { type: 'stream-start'; warnings: Array<SharedV4Warning> }
  | { type: 'response-metadata' } & LanguageModelV4ResponseMetadata
  | { type: 'finish'; usage: LanguageModelV4Usage; finishReason: LanguageModelV4FinishReason; providerMetadata? }

  // Text blocks — emit text-start before any text-delta, text-end after last delta
  | { type: 'text-start'; id: string; providerMetadata? }
  | { type: 'text-delta'; id: string; delta: string; providerMetadata? }
  | { type: 'text-end';   id: string; providerMetadata? }

  // Reasoning blocks — same start/delta/end pattern
  | { type: 'reasoning-start'; id: string; providerMetadata? }
  | { type: 'reasoning-delta'; id: string; delta: string; providerMetadata? }
  | { type: 'reasoning-end';   id: string; providerMetadata? }

  // Tool calls — streamed incrementally
  | { type: 'tool-input-start'; id: string; toolName: string; providerMetadata?; providerExecuted?; dynamic?; title? }
  | { type: 'tool-input-delta'; id: string; delta: string; providerMetadata? }
  | { type: 'tool-input-end';   id: string; providerMetadata? }
  | LanguageModelV4ToolApprovalRequest
  | LanguageModelV4ToolCall    // { type: 'tool-call'; ... }  emitted after tool-input-end
  | LanguageModelV4ToolResult
  | LanguageModelV4CustomContent

  // Files and sources
  | LanguageModelV4File
  | LanguageModelV4ReasoningFile
  | LanguageModelV4Source

  // Raw pass-through (when includeRawChunks is true)
  | { type: 'raw'; rawValue: unknown }

  // Errors (can be multiple; do not abort the stream)
  | { type: 'error'; error: unknown };
```

**Critical ordering rules** (from `openai-compatible-chat-language-model.ts` implementation):
1. `stream-start` must be the **first** chunk emitted (`start(controller)` in TransformStream)
2. Reasoning blocks must close before text blocks start
3. Text blocks need `text-start` before any `text-delta`
4. `finish` must be emitted **last**
5. Error chunks can be emitted mid-stream; they do not close the stream

### Error Handling Contract

Source: `packages/provider/src/errors/api-call-error.ts`, `packages/provider-utils/src/post-to-api.ts`

The base error class is `AISDKError extends Error` with a `cause?` field and a Symbol-keyed marker for cross-package `instanceof` checks.

`APICallError extends AISDKError` carries:
```typescript
class APICallError extends AISDKError {
  readonly url: string;
  readonly requestBodyValues: unknown;
  readonly statusCode?: number;
  readonly responseHeaders?: Record<string, string>;
  readonly responseBody?: string;
  readonly isRetryable: boolean;  // computed from statusCode
  readonly data?: unknown;
}
```

**`isRetryable` is set to `true` automatically for status codes: 408, 409, 429, and any >= 500.** HTTP 402 is **not** in this list, so the SDK will **not** auto-retry a 402. This is the correct behavior for Solvela's use case — the 402 must be caught, signed, and retried by the provider itself.

Other typed errors exported from `@ai-sdk/provider`:
- `InvalidArgumentError`
- `InvalidPromptError`
- `InvalidResponseDataError`
- `LoadAPIKeyError`
- `NoContentGeneratedError`
- `NoSuchModelError`
- `TypeValidationError`
- `UnsupportedFunctionalityError`
- `JSONParseError`
- `EmptyResponseBodyError`

### How SDK APIs Drive the Provider

- `generateText` / `generateObject` → calls `model.doGenerate(options)`, awaits the result
- `streamText` / `streamObject` → calls `model.doStream(options)`, consumes the `ReadableStream<LanguageModelV4StreamPart>`
- Tool calling: the SDK passes `tools` and `toolChoice` in `CallOptions`; the provider emits `tool-input-*` + `tool-call` stream parts; the SDK executes the tool and feeds results back via re-invocation
- `generateObject`: if `responseFormat: { type: 'json', schema }` is passed and the provider has `supportsStructuredOutputs: true`, the provider handles JSON mode natively; otherwise the SDK wraps the text output and parses it itself

---

## Section 2: Existing Provider Reference Implementations

### `@ai-sdk/openai-compatible` — The Key Package

Source: `packages/openai-compatible/` in [github.com/vercel/ai](https://github.com/vercel/ai)

This package **exists** and is the primary mechanism for wrapping any OpenAI-format endpoint. It is not first-party to a specific vendor; it is a generic adapter.

**Public surface of `createOpenAICompatible`:**

```typescript
interface OpenAICompatibleProviderSettings {
  baseURL: string;                    // Required
  name: string;                       // Required — sets provider ID prefix
  apiKey?: string;                    // Adds Authorization: Bearer <apiKey>
  headers?: Record<string, string>;  // Static extra headers
  queryParams?: Record<string, string>;
  fetch?: FetchFunction;              // CUSTOM FETCH — key injection point
  includeUsage?: boolean;
  supportsStructuredOutputs?: boolean;
  transformRequestBody?: (args: Record<string, any>) => Record<string, any>;  // Body transform hook
  metadataExtractor?: MetadataExtractor;
}
```

The `fetch` field accepts any function matching `(url: string, init?: RequestInit) => Promise<Response>` — this is the primary hook for intercepting HTTP calls.

The `transformRequestBody` hook allows mutating the JSON body before transmission.

**File layout:**
```
packages/openai-compatible/src/
├── openai-compatible-provider.ts       # createOpenAICompatible factory
├── openai-compatible-error.ts          # error structure
├── chat/
│   ├── openai-compatible-chat-language-model.ts   # implements LanguageModelV4
│   ├── convert-to-openai-compatible-chat-messages.ts
│   ├── openai-compatible-prepare-tools.ts
│   ├── openai-compatible-chat-options.ts
│   └── openai-compatible-metadata-extractor.ts
├── completion/
├── embedding/
├── image/
└── utils/
```

**How doGenerate works** (from source):
1. Calls `this.getArgs()` to build the OpenAI-format request body
2. Calls `postJsonToApi` with `url`, `headers`, the body, and two handlers: `failedResponseHandler` (for non-2xx) and `successfulResponseHandler`
3. Maps the response to `LanguageModelV4GenerateResult`

**How doStream works:**
1. Same arg-building step, adds `stream: true`
2. Calls `postJsonToApi` with `createEventSourceResponseHandler` for SSE parsing
3. Returns a `ReadableStream` fed through a `TransformStream` that maps SSE chunks to `LanguageModelV4StreamPart` objects
4. Emits `stream-start` in the TransformStream's `start()` hook

**Tool call handling:** The `prepareTools` utility maps `LanguageModelV4FunctionTool[]` to OpenAI's `{type:'function', function:{name,description,parameters}}` format. The streaming handler tracks partial tool call arguments by index and emits `tool-input-start` / `tool-input-delta` / `tool-input-end` / `tool-call` in sequence.

**Error handling:** A `failedResponseHandler` (built from `createJsonErrorResponseHandler`) reads non-2xx responses and creates an `APICallError`. **Any response that is not 2xx, including 402, goes through this handler and results in a thrown `APICallError`.**

### `@ai-sdk/openai`

Source: `packages/openai/src/` — implements `LanguageModelV4` directly, structurally identical to `openai-compatible` but with OpenAI-specific field names, models, and beta features (structured outputs, vision, etc.).

### `@ai-sdk/anthropic`

Source: `packages/anthropic/src/` — implements `LanguageModelV4` with Anthropic's native message format. Key difference: it uses a custom SSE event structure (`message_start`, `content_block_start`, etc.) and converts to `LanguageModelV4StreamPart` in a TransformStream.

### `@ai-sdk/google`

Source: `packages/google/src/` — implements `LanguageModelV4`. Uses Google's `generateContent`/`streamGenerateContent` REST API rather than OpenAI format.

### Recommendation for Solvela

Because Solvela's gateway speaks OpenAI-compatible wire format, **the correct approach is to call `createOpenAICompatible` and inject a custom `fetch` function** that implements the 402→sign→retry loop. There is no need to implement `LanguageModelV4` from scratch.

If more control is needed over the provider ID, model enumeration, or initialization API, the pattern is to write a thin `createSolvelaProvider` factory that calls `createOpenAICompatible` internally with a fixed `baseURL` and custom `fetch`. See §4 for the precise interception seam.

---

## Section 3: Publishing and Distribution

### Package Naming

The `@ai-sdk/` npm scope is **owned by Vercel** and restricted to first-party packages. Community providers cannot publish under `@ai-sdk/`.

Community provider naming conventions observed across the ecosystem:

| Pattern | Example |
|---|---|
| `ai-sdk-provider-<name>` (unscoped) | `ai-sdk-provider-claude-code`, `ai-sdk-provider-opencode-sdk` |
| `@<org>/ai-sdk-provider` | `@mem0/vercel-ai-provider` |
| `@<org>/ai-sdk` | `@requesty/ai-sdk` |
| `@openrouter/ai-sdk-provider` | scoped under vendor org |

For Solvela, `@solvela/ai-sdk-provider` or `ai-sdk-provider-solvela` are both idiomatic.

Sources:
- [Community Providers index](https://ai-sdk.dev/providers/community-providers/custom-providers)
- [github.com/ben-vargas/ai-sdk-provider-claude-code](https://github.com/ben-vargas/ai-sdk-provider-claude-code)
- [github.com/ben-vargas/ai-sdk-provider-opencode-sdk](https://github.com/ben-vargas/ai-sdk-provider-opencode-sdk)

### Provider Registry / Listing

There is no automated registry. The process is:
1. Publish the npm package
2. Submit a PR to `vercel/ai` adding a page under `content/providers/03-community-providers/`

Source: [https://ai-sdk.dev/providers/community-providers/custom-providers](https://ai-sdk.dev/providers/community-providers/custom-providers)

### `peerDependencies` Pattern

Community providers declare `ai` and `@ai-sdk/provider` as **peer dependencies**, not direct dependencies. Looking at `@ai-sdk/openai-compatible`:

```json
{
  "dependencies": {
    "@ai-sdk/provider": "workspace:*",
    "@ai-sdk/provider-utils": "workspace:*"
  },
  "peerDependencies": {
    "zod": "^3.25.76 || ^4.1.8"
  }
}
```

For a community package, the convention is:
```json
{
  "peerDependencies": {
    "ai": ">=6.0.0",
    "@ai-sdk/provider": ">=3.0.0"
  },
  "dependencies": {
    "@ai-sdk/provider-utils": "^4.0.0"  // for fetch utilities
  }
}
```

### ESM-Only, Node Version, TypeScript Target

- **ESM-only**: As of `@ai-sdk/openai-compatible` 3.0.0-beta.24 / `@ai-sdk/provider` 4.0.0-beta.11, **all packages dropped CommonJS exports**. CHANGELOG entry: `"ef992f8: Remove CommonJS exports from all packages. All packages are now ESM-only ('type': 'module'). Consumers using require() must switch to ESM import syntax."`
- **Node version**: `"engines": { "node": ">=18" }` in all packages
- **TypeScript**: `typescript: "5.8.3"` in devDependencies; target is ESM module output via `tsup`
- Dual-export (CJS+ESM): **no longer supported** in beta channel; stable v6 packages still ship dual-format

Sources:
- `packages/openai-compatible/package.json`
- `packages/provider/CHANGELOG.md`

---

## Section 4: Gotchas Specific to the 402/Sign/Retry Flow

### Where to Intercept the 402

The entire `postToApi` flow in `@ai-sdk/provider-utils` is:

```
fetch(url, init)
  → response.ok? → successfulResponseHandler(response)
  → !response.ok → failedResponseHandler(response) → throw APICallError
```

Source: `packages/provider-utils/src/post-to-api.ts`

**A 402 response causes `failedResponseHandler` to run, which throws an `APICallError`.** There is no hook between "response received" and "error thrown" in the standard `postJsonToApi` utility.

**The correct interception point is the `fetch` parameter of `createOpenAICompatible`.** The `fetch` parameter is a `FetchFunction` — a standard `(url, init) => Promise<Response>` — that replaces `globalThis.fetch`. By providing a custom fetch, you can:

1. Make the first call normally
2. Inspect the response status before it reaches `postToApi`'s `!response.ok` branch
3. If status is 402: read the response body (cost + payment schemes), call a user-supplied signing callback, retry with `PAYMENT-SIGNATURE` header added to `init.headers`
4. Return the signed response to `postToApi`, which then sees a 2xx and proceeds normally

```typescript
function createSolvelaFetch(signer: SolvelaWalletSigner): FetchFunction {
  return async (url, init) => {
    const firstResponse = await globalThis.fetch(url, init);
    if (firstResponse.status !== 402) return firstResponse;

    // Parse 402 body for cost and payment schemes
    const paymentRequired = await firstResponse.json();
    const signature = await signer.sign(paymentRequired);

    // Retry with signed header
    const retryInit = {
      ...init,
      headers: {
        ...(init?.headers as Record<string, string>),
        'PAYMENT-SIGNATURE': signature,
      },
    };
    return globalThis.fetch(url, retryInit);
  };
}
```

This fetch wrapper is then passed as `fetch` to `createOpenAICompatible`. No modification to SDK internals is required.

**Alternative: `LanguageModelV4Middleware`**

The `wrapLanguageModel` + `LanguageModelV4Middleware` pattern is another valid seam. The `wrapGenerate` and `wrapStream` hooks receive the `doGenerate` / `doStream` functions and can call them in a try/catch:

```typescript
const solvelaMiddleware: LanguageModelV4Middleware = {
  specificationVersion: 'v4',
  async wrapGenerate({ doGenerate, params }) {
    try {
      return await doGenerate();
    } catch (err) {
      if (APICallError.isInstance(err) && err.statusCode === 402) {
        // parse err.responseBody, sign, add header to params, retry
        // BUT: params.headers is passed to the model, which then passes to postJsonToApi
        // This works if the underlying model respects options.headers
        const signature = await signer.sign(JSON.parse(err.responseBody!));
        return await doGenerate(); // need to inject header — see caveat below
      }
      throw err;
    }
  }
};
```

**Caveat for the middleware approach:** `wrapGenerate` receives a `doGenerate: () => Promise<...>` closure that has already captured its `params`. To inject a header for the retry, you would need to call `model.doGenerate({ ...params, headers: { ...params.headers, 'PAYMENT-SIGNATURE': sig } })` directly rather than calling the pre-bound `doGenerate()`. The middleware receives `model` as a parameter, so this is possible. However, the custom `fetch` approach is simpler and keeps the signing logic fully encapsulated.

Source: `packages/provider/src/language-model-middleware/v4/language-model-v4-middleware.ts`, `packages/ai/src/middleware/wrap-language-model.ts`

### Does the SDK Auto-Retry on 402?

**No.** The `isRetryable` flag in `APICallError` is only `true` for status codes 408, 409, 429, and >= 500. 402 is explicitly not retried. The 402 will surface as a thrown `APICallError` with `statusCode: 402`. The Solvela provider must implement the retry itself, which the custom `fetch` wrapper above does cleanly.

Source: `packages/provider/src/errors/api-call-error.ts`, constructor `isRetryable` default computation

### Setting Arbitrary Per-Request Headers

`LanguageModelV4CallOptions.headers` is a `Record<string, string | undefined>` that flows directly into `combineHeaders(this.config.headers?.(), options.headers)` in the chat model's `doGenerate`/`doStream`, then into `postJsonToApi`. So callers can pass `PAYMENT-SIGNATURE` as a top-level header at call time:

```typescript
await generateText({
  model: solvela('claude-sonnet-4-5'),
  prompt: '...',
  headers: { 'PAYMENT-SIGNATURE': precomputedSig },
});
```

But for the automatic 402→sign→retry flow (invisible to callers), the custom `fetch` wrapper is the right place because it intercepts at the HTTP layer before the SDK processes the response.

### Streaming with Retry After 402

The streaming path uses `createEventSourceResponseHandler` — the SSE stream starts only after the HTTP response headers arrive with a 2xx status. A 402 arrives **before any SSE data flows**, so the retry is clean: no partial stream state to clean up. The custom `fetch` wrapper intercepts the 402 before the SDK's `successfulResponseHandler` (which sets up SSE parsing) ever runs. From the SDK's perspective, only the successful 200 response exists.

This means the retry is transparent to the `TransformStream` layer that maps SSE chunks to `LanguageModelV4StreamPart`. No SDK primitives beyond the `fetch` injection are needed.

### Tool-Calling JSON Shape

The `@ai-sdk/openai-compatible` `doGenerate` maps OpenAI's `tool_calls` array:
```
{ id, type: 'function', function: { name, arguments } }
```
to:
```typescript
{ type: 'tool-call', toolCallId: id, toolName: name, input: arguments }
```

`input` is a raw JSON string (not parsed). The SDK parses it against the tool's `inputSchema`. Solvela's upstream returns standard OpenAI tool_calls format, so no additional normalization is needed.

Source: `packages/openai-compatible/src/chat/openai-compatible-chat-language-model.ts`, `doGenerate` tool_calls section

For streaming, `tool-input-start` / `tool-input-delta` / `tool-input-end` / `tool-call` are emitted in sequence as the JSON argument string accumulates. The `isParsableJson` check is used to detect single-chunk full tool calls (some providers send complete args in one SSE chunk).

### `generateObject` / Structured Output

The `openai-compatible` provider has `supportsStructuredOutputs?: boolean` (defaults to `false`). When `false` and `responseFormat.type === 'json'`, the provider sends `response_format: { type: 'json_object' }` and emits a warning that schema-constrained JSON is unsupported. The AI SDK core then parses and validates the text output against the schema itself.

When `supportsStructuredOutputs: true`, it sends `response_format: { type: 'json_schema', json_schema: { schema, strict: true, name } }`. Solvela's upstream (being OpenAI-compatible) likely supports this. Set `supportsStructuredOutputs: true` in the provider config.

### Multimodal Input

The prompt type `LanguageModelV4FilePart` (user role content):

```typescript
interface LanguageModelV4FilePart {
  type: 'file';
  filename?: string;
  data: LanguageModelV4DataContent | SharedV4ProviderReference;
  mediaType: string;   // IANA type, e.g. "image/png", "application/pdf", "image/*"
}
```

`LanguageModelV4DataContent` is `string | Uint8Array` (base64 string or binary).

`supportedUrls` on the provider controls which URL-based files are passed as-is vs. downloaded and inlined by the SDK. If `supportedUrls` returns `{}` (empty, the default in `openai-compatible`), the SDK downloads all remote files and inlines them as base64. If you declare `{ 'image/*': [/.*/] }`, the SDK passes image URLs directly.

Source: `packages/provider/src/language-model/v4/language-model-v4-prompt.ts`

---

## Section 5: API Stability

### Version History and Breaking Changes (Last 12 Months)

Source: `packages/provider/CHANGELOG.md` in the monorepo

| Version | Breaking Change |
|---|---|
| `3.0.0` (stable) | `LanguageModelV3` introduced (AI SDK 6 beta launch) |
| `3.0.0` | `LanguageModelV3ToolResult["result"]` changed from `unknown` to `NonNullable<JSONValue>` |
| `3.0.0` | `providerExecuted` removed from `LanguageModelV3ToolResult` |
| `4.0.0-beta.0` | V7 pre-release started; `LanguageModelV4` added |
| `4.0.0-beta.3` | Added `'custom'` content type to V4 spec |
| `4.0.0-beta.11` | **ESM-only**: CommonJS exports removed from all packages |
| `4.0.0-beta.12` | `image-*` tool output types merged into `file-*` types |

The interface has shipped **one major breaking migration per SDK major version**: V1→V2→V3→V4, roughly aligned with `ai` v3→v4→v5→v6→v7. The cadence is approximately one major per 9–12 months.

### V3 vs V4: Which to Target?

The monorepo's `wrapLanguageModel` explicitly accepts `LanguageModelV2 | LanguageModelV3 | LanguageModelV4`:

```typescript
export const wrapLanguageModel = ({
  model: inputModel,
  ...
}: {
  model: LanguageModelV2 | LanguageModelV3 | LanguageModelV4;
  ...
}): LanguageModelV4 => {
  const model = asLanguageModelV4(inputModel);  // upconverts V2/V3 to V4
  ...
};
```

Source: `packages/ai/src/middleware/wrap-language-model.ts`

**Recommendation:** Implement `LanguageModelV4` (`specificationVersion: 'v4'`) now. The `@ai-sdk/openai-compatible` package on main already uses V4. V4 and V3 are structurally identical — the only migration cost if the stable release catches up is changing the `specificationVersion` literal. There is a forward-compatibility adapter (`asLanguageModelV4`) that handles V2/V3 models used in V4 contexts.

V4 is currently in pre-release (`@ai-sdk/provider@4.0.0-beta.12`, `ai@7-pre`). If shipping against `ai@6` stable today, implement `LanguageModelV3`. If shipping against the v7 beta, implement `LanguageModelV4`.

---

## Known Gaps

1. **`ai` v7 stable release date**: The v7 pre-release is active on main. No public timeline for stable v7 was found.

2. **`APICallError.data` field on 402**: The `data?` field on `APICallError` could carry the parsed 402 JSON body. Whether `defaultOpenAICompatibleErrorStructure` (in `openai-compatible-error.ts`) parses and populates this field for 402 responses was not verified — only the error class definition was inspected. If it does, the middleware-based interception could read `err.data` instead of re-reading `err.responseBody`.

3. **Provider-utils `FetchFunction` exact typedef**: The file `packages/provider-utils/src/fetch-function.ts` was not read; the type is presumed to be `(url: string, init?: RequestInit) => Promise<Response>` based on usage in `postToApi`.

4. **`@ai-sdk/openai-compatible` stable version number**: The monorepo shows `3.0.0-beta.26`; the corresponding stable `2.x` version was not verified on npm to confirm the stable-channel API is identical.

5. **Community provider listing process**: The exact PR format and review SLA for getting listed on `ai-sdk.dev/providers/community-providers` was not verified beyond the general description that a PR to the `content/` directory is required.

6. **`isRetryable` behavior within `ai` core**: Whether the `ai` package's `generateText` / `streamText` functions perform their own automatic retry loop based on `APICallError.isRetryable` was not traced — only the `APICallError` class definition confirms 402 is marked non-retryable at the error level. If the `ai` core does have a retry loop, 402 would not trigger it.

---

### Primary Sources

- Vercel AI SDK monorepo: [https://github.com/vercel/ai](https://github.com/vercel/ai)
- `packages/provider/src/language-model/v4/language-model-v4.ts` — main interface
- `packages/provider/src/language-model/v4/language-model-v4-call-options.ts` — call options
- `packages/provider/src/language-model/v4/language-model-v4-stream-part.ts` — stream parts
- `packages/provider/src/language-model/v4/language-model-v4-generate-result.ts` — generate result
- `packages/provider/src/language-model-middleware/v4/language-model-v4-middleware.ts` — middleware interface
- `packages/provider/src/errors/api-call-error.ts` — error class with isRetryable logic
- `packages/provider-utils/src/post-to-api.ts` — HTTP layer, 402 interception point
- `packages/openai-compatible/src/openai-compatible-provider.ts` — createOpenAICompatible factory
- `packages/openai-compatible/src/chat/openai-compatible-chat-language-model.ts` — doGenerate/doStream reference implementation
- `packages/ai/src/middleware/wrap-language-model.ts` — wrapLanguageModel utility
- `packages/provider/CHANGELOG.md` — version history
- [AI SDK 6 announcement](https://vercel.com/blog/ai-sdk-6)
- [Custom providers docs](https://ai-sdk.dev/providers/community-providers/custom-providers)
- [Provider management docs](https://ai-sdk.dev/docs/ai-sdk-core/provider-management)
