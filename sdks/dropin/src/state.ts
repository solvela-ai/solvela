/**
 * Sidecar state file (`~/.solvela/dropin.json`, mode 0600).
 *
 * Holds the gateway URL, channel id, the delegated channel SESSION SEED (a
 * secret — never logged, never printed; it lives ONLY in this 0600 file), the
 * local bearer token Claude Code presents, and a best-effort `last_cumulative`
 * (the gateway's ledger is ground truth — on restart the tracker's structured
 * resync self-heals from any stale value, so losing this field costs one
 * round-trip, never money).
 */

import { randomBytes } from 'node:crypto';
import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';

export interface DropinState {
  gateway_url: string;
  port: number;
  /** base58 32-byte channel id. */
  channel_id: string;
  /** base58 32-byte ed25519 session seed. SECRET — never log or print. */
  session_seed_b58: string;
  /** The local bearer token (`ANTHROPIC_AUTH_TOKEN`). Local-only credential. */
  local_token: string;
  /** Best-effort last accepted cumulative (atomic string). Gateway is truth. */
  last_cumulative: string;
}

export function defaultStatePath(): string {
  return path.join(os.homedir(), '.solvela', 'dropin.json');
}

/**
 * Write the state file at mode 0600 (dir 0700), via a same-directory temp +
 * atomic rename so a crash mid-write never leaves a partial file where the
 * seed lives.
 */
export async function saveState(statePath: string, state: DropinState): Promise<void> {
  const dir = path.dirname(statePath);
  await fs.mkdir(dir, { recursive: true, mode: 0o700 });
  const tmp = path.join(dir, `.dropin.json.tmp-${process.pid}-${randomBytes(6).toString('hex')}`);
  await fs.writeFile(tmp, JSON.stringify(state, null, 2), { mode: 0o600 });
  try {
    await fs.rename(tmp, statePath);
  } catch (err) {
    await fs.unlink(tmp).catch(() => {});
    throw err;
  }
  if (process.platform !== 'win32') {
    // Belt-and-braces: writeFile's mode can be widened by the umask on some
    // platforms; re-assert on the final path.
    await fs.chmod(statePath, 0o600).catch(() => {});
  }
}

/**
 * Load + validate the state file. Fails closed with field-naming errors that
 * NEVER echo the seed or token values.
 */
export async function loadState(statePath: string): Promise<DropinState> {
  let raw: string;
  try {
    raw = await fs.readFile(statePath, 'utf-8');
  } catch {
    throw new Error(
      `no sidecar state at ${statePath} — run \`solvela-dropin open\` first to fund a channel`,
    );
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error(`state file ${statePath} is not valid JSON`);
  }
  if (typeof parsed !== 'object' || parsed === null) {
    throw new Error(`state file ${statePath} is not a JSON object`);
  }
  const obj = parsed as Record<string, unknown>;
  const requireString = (field: string): string => {
    const v = obj[field];
    if (typeof v !== 'string' || v === '') {
      throw new Error(`state file ${statePath} is missing a string '${field}' field`);
    }
    return v;
  };
  const port = obj['port'];
  if (typeof port !== 'number' || !Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new Error(`state file ${statePath} has an invalid 'port' field`);
  }
  const lastCumulative = requireString('last_cumulative');
  if (!/^\d+$/.test(lastCumulative)) {
    throw new Error(`state file ${statePath} has a non-integer 'last_cumulative' field`);
  }
  return {
    gateway_url: requireString('gateway_url'),
    port,
    channel_id: requireString('channel_id'),
    session_seed_b58: requireString('session_seed_b58'),
    local_token: requireString('local_token'),
    last_cumulative: lastCumulative,
  };
}
