/**
 * Runtime validation for MCP tool call arguments.
 *
 * MCP tool schemas (`inputSchema` JSON Schema on Tool definitions) are
 * advisory — the SDK does not enforce them before the server's
 * `CallToolRequestSchema` handler runs. A misbehaving or upgraded host
 * can therefore call a tool with arguments that disagree with the schema
 * (wrong types, missing required fields, profile values outside the
 * declared enum). The handler used to `args as { ... }` blind-cast and
 * propagate junk values downstream (e.g. `client.chat(42, ...)`).
 *
 * This helper performs minimal, message-shaped checks and throws
 * `McpError(InvalidParams, ...)` on mismatch so the host gets a clean
 * JSON-RPC error instead of a downstream surprise.
 *
 * Scope:
 *   - string / number / boolean primitives
 *   - string enums
 *   - required vs optional
 *
 * NOT covered (call-site concerns):
 *   - inter-field invariants (e.g. amount > 0)
 *   - format validation (regex)
 *   - the deposit_escrow tool already does its own stricter check on
 *     `amount_usdc` and is not migrated here
 */

import { ErrorCode, McpError } from '@modelcontextprotocol/sdk/types.js';

export type FieldSpec =
  | { kind: 'string'; required: boolean }
  | { kind: 'number'; required: boolean }
  | { kind: 'boolean'; required: boolean }
  | { kind: 'stringEnum'; required: boolean; values: readonly string[] }
  | { kind: 'stringArray'; required: boolean };

export type Schema = Record<string, FieldSpec>;

/**
 * Validate `args` against `schema` and return it as the typed shape.
 *
 * Throws `McpError(InvalidParams, ...)` if:
 *   - `args` is not a plain object
 *   - a required field is missing or `undefined`
 *   - a present field has the wrong type
 *   - a stringEnum field has a value outside the declared values
 *
 * Optional fields that are `undefined` pass through (the consumer's
 * destructuring default — e.g. `profile = 'auto'` — still applies).
 */
export function validateArgs<T>(toolName: string, args: unknown, schema: Schema): T {
  if (typeof args !== 'object' || args === null || Array.isArray(args)) {
    throw new McpError(
      ErrorCode.InvalidParams,
      `${toolName}: arguments must be an object, got ${args === null ? 'null' : Array.isArray(args) ? 'array' : typeof args}`,
    );
  }
  const obj = args as Record<string, unknown>;
  for (const [field, spec] of Object.entries(schema)) {
    const val = obj[field];
    if (val === undefined) {
      if (spec.required) {
        throw new McpError(
          ErrorCode.InvalidParams,
          `${toolName}: missing required field '${field}'`,
        );
      }
      continue;
    }
    switch (spec.kind) {
      case 'string':
        if (typeof val !== 'string') {
          throw new McpError(
            ErrorCode.InvalidParams,
            `${toolName}: field '${field}' must be a string, got ${typeof val}`,
          );
        }
        break;
      case 'number':
        // Reject NaN/Infinity. The JSON parser cannot produce these directly,
        // but a Number(...) coercion upstream could.
        if (typeof val !== 'number' || !Number.isFinite(val)) {
          throw new McpError(
            ErrorCode.InvalidParams,
            `${toolName}: field '${field}' must be a finite number, got ${typeof val === 'number' ? String(val) : typeof val}`,
          );
        }
        break;
      case 'boolean':
        if (typeof val !== 'boolean') {
          throw new McpError(
            ErrorCode.InvalidParams,
            `${toolName}: field '${field}' must be a boolean, got ${typeof val}`,
          );
        }
        break;
      case 'stringEnum':
        if (typeof val !== 'string' || !spec.values.includes(val)) {
          throw new McpError(
            ErrorCode.InvalidParams,
            `${toolName}: field '${field}' must be one of ${spec.values.map((v) => `'${v}'`).join('|')}, got ${JSON.stringify(val)}`,
          );
        }
        break;
      case 'stringArray':
        // Type-shape only (array of strings); content rules (length bounds,
        // emptiness) are call-site concerns per this module's scope note.
        if (!Array.isArray(val) || !val.every((v) => typeof v === 'string')) {
          throw new McpError(
            ErrorCode.InvalidParams,
            `${toolName}: field '${field}' must be an array of strings`,
          );
        }
        break;
    }
  }
  return obj as T;
}
