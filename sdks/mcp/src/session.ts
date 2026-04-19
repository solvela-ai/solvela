/**
 * Session persistence for the Solvela MCP server.
 *
 * Reads and writes ~/.solvela/mcp-session.json atomically to survive
 * process restarts. Schema version 1.
 *
 * Concurrency: the store itself does NOT serialize writes — the caller
 * (GatewayClient) is responsible via its budgetMutex.
 */

import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';

export interface SessionState {
  /** Cumulative USDC spent via chat/smart_chat this session. */
  session_spent: number;
  /** Cumulative USDC deposited via deposit_escrow this session. */
  escrow_deposits_session: number;
  /** Total requests this session. */
  request_count: number;
  /** ISO timestamp of last update. */
  last_updated: string;
  /** Schema version — must be 1. */
  version: number;
}

export interface SessionStore {
  /** Load state from disk. Returns default state if file is missing or invalid. */
  load(): Promise<SessionState>;
  /** Atomically write state to disk. */
  save(state: SessionState): Promise<void>;
  /** Delete the session file (resets persistence). */
  reset(): Promise<void>;
  /** Absolute path to the session file. */
  path(): string;
}

const CURRENT_VERSION = 1;

function defaultState(): SessionState {
  return {
    session_spent: 0,
    escrow_deposits_session: 0,
    request_count: 0,
    last_updated: new Date().toISOString(),
    version: CURRENT_VERSION,
  };
}

/**
 * Validate loaded state. Returns true if the state is usable.
 * Logs a WARN to stderr on validation failures.
 */
function validateState(raw: unknown, filePath: string): SessionState | null {
  if (typeof raw !== 'object' || raw === null) {
    process.stderr.write(
      `[solvela-mcp] WARN: session file ${filePath} is not an object — resetting to defaults\n`,
    );
    return null;
  }

  const s = raw as Record<string, unknown>;

  // Version check
  if (s['version'] !== CURRENT_VERSION) {
    process.stderr.write(
      `[solvela-mcp] WARN: session file ${filePath} has unknown version ${String(s['version'])} — resetting to defaults\n`,
    );
    return null;
  }

  // Validate numeric fields
  const numericFields = ['session_spent', 'escrow_deposits_session', 'request_count'] as const;
  for (const field of numericFields) {
    const val = s[field];
    if (typeof val !== 'number' || !Number.isFinite(val) || val < 0) {
      process.stderr.write(
        `[solvela-mcp] WARN: session file ${filePath} has invalid ${field}=${String(val)} — resetting to defaults\n`,
      );
      return null;
    }
  }

  // Validate last_updated — must be a non-empty string parseable as an ISO date.
  const lastUpdated = s['last_updated'];
  if (
    typeof lastUpdated !== 'string' ||
    !lastUpdated ||
    Number.isNaN(Date.parse(lastUpdated))
  ) {
    process.stderr.write(
      `[solvela-mcp] WARN: session file ${filePath} has invalid last_updated=${JSON.stringify(lastUpdated)} — resetting to defaults\n`,
    );
    return null;
  }

  return {
    session_spent: s['session_spent'] as number,
    escrow_deposits_session: s['escrow_deposits_session'] as number,
    request_count: s['request_count'] as number,
    last_updated: s['last_updated'] as string,
    version: CURRENT_VERSION,
  };
}

function createSessionStoreImpl(filePath: string): SessionStore {
  return {
    path(): string {
      return filePath;
    },

    async load(): Promise<SessionState> {
      let raw: string;
      try {
        raw = await fs.readFile(filePath, 'utf-8');
      } catch (err) {
        // File missing — fresh session
        if ((err as NodeJS.ErrnoException).code === 'ENOENT') {
          return defaultState();
        }
        process.stderr.write(
          `[solvela-mcp] WARN: failed to read session file ${filePath}: ${err instanceof Error ? err.message : String(err)} — resetting to defaults\n`,
        );
        return defaultState();
      }

      let parsed: unknown;
      try {
        parsed = JSON.parse(raw);
      } catch {
        process.stderr.write(
          `[solvela-mcp] WARN: session file ${filePath} is not valid JSON — resetting to defaults\n`,
        );
        return defaultState();
      }

      // H6: Opportunistic orphan cleanup — remove any leftover .tmp from a prior crash.
      await fs.unlink(`${filePath}.tmp`).catch(() => {});

      return validateState(parsed, filePath) ?? defaultState();
    },

    async save(state: SessionState): Promise<void> {
      const dir = path.dirname(filePath);

      // Ensure parent directory exists.
      await fs.mkdir(dir, { recursive: true, mode: 0o700 });

      // H8: chmod the directory after mkdir — mkdir({mode}) does NOT apply the mode
      // to a pre-existing directory on Linux. chmod is a no-op on Windows (silently ignored).
      if (process.platform !== 'win32') {
        await fs.chmod(dir, 0o700).catch(() => {});
      }

      const json = JSON.stringify(state, null, 2);
      const tmpPath = `${filePath}.tmp`;

      // Write to .tmp first, then atomically rename.
      // H6: On any error after writeFile, unlink the .tmp to avoid orphans.
      try {
        await fs.writeFile(tmpPath, json, { encoding: 'utf-8', mode: 0o600 });

        // Set file permissions on Unix (Windows skips chmod silently)
        if (process.platform !== 'win32') {
          await fs.chmod(tmpPath, 0o600);
        }

        await fs.rename(tmpPath, filePath);
      } catch (err) {
        // Clean up the .tmp orphan; ignore errors (e.g. writeFile never created it)
        await fs.unlink(tmpPath).catch(() => {});
        throw err;
      }
    },

    async reset(): Promise<void> {
      try {
        await fs.unlink(filePath);
      } catch (err) {
        // ENOENT is fine — file already gone
        if ((err as NodeJS.ErrnoException).code !== 'ENOENT') {
          throw err;
        }
      }
      // H6: Also unlink any .tmp orphan left by a prior crash during save().
      await fs.unlink(`${filePath}.tmp`).catch(() => {});
    },
  };
}

/**
 * Create a SessionStore.
 *
 * @param opts.path  Override the default session file path (useful for testing).
 *                   Default: ~/.solvela/mcp-session.json
 */
export function createSessionStore(opts?: { path?: string }): SessionStore {
  const filePath =
    opts?.path ?? path.join(os.homedir(), '.solvela', 'mcp-session.json');
  return createSessionStoreImpl(filePath);
}
