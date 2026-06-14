/**
 * Non-custodial wallet resolution + auto-creation for the Solvela MCP server.
 *
 * The MCP server used to hard-exit when `SOLANA_WALLET_KEY` was unset. This
 * module resolves — and, on first run, auto-creates — a client-side wallet so a
 * new user never has to generate a keypair or set an env var. The security model
 * is unchanged: the private key stays on the machine and only ever reaches the
 * signer via the same `SOLANA_WALLET_KEY`-shaped base58 string; only signed
 * transactions leave the host.
 *
 * Resolution precedence (highest first):
 *   1. `SOLANA_WALLET_KEY` env (base58 64-byte secret) — explicit, wins. When
 *      set, NO file is read or created.
 *   2. `<baseDir>/wallet.json` field `private_key` — this file is SHARED with
 *      the Rust CLI (`crates/cli/src/commands/wallet.rs`). Read if present.
 *   3. Neither → auto-generate an ed25519 keypair and write `<baseDir>/wallet.json`.
 *
 * On-disk schema (byte-compatible with the Rust CLI's `wallet init`):
 *   { "private_key": "<base58 of 64 bytes: seed[0..32] || pubkey[32..64]>",
 *     "address":     "<base58 of the 32-byte pubkey>",
 *     "created_at":  "<RFC3339 timestamp>" }
 *
 * The 64-byte base58 form is exactly what `SOLANA_WALLET_KEY` and the signer
 * (`Keypair.fromSecretKey(bs58.decode(...))` in signer-core) already expect, so
 * a resolved key plugs into the signing path identically.
 *
 * SECURITY:
 *  - The private key is a secret. It is NEVER logged, printed, or embedded in a
 *    notice/error — only the public address is surfaced.
 *  - The wallet file is written 0600; the parent dir is 0700.
 *  - Auto-create is atomic and exclusive (write a unique temp, fchmod 0600 on the
 *    FD before close, then rename), and NEVER overwrites an existing wallet.json.
 *  - On load, the file is validated: the decoded `private_key` must be exactly
 *    64 bytes and the pubkey derived from it must equal the stored `address`.
 *    A corrupt/tampered file fails closed (throws) — it is never silently used.
 */

import { generateKeyPairSync, randomBytes } from 'node:crypto';
import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';

import { Keypair } from '@solana/web3.js';
import bs58 from 'bs58';

/** Signing modes accepted by the MCP server (mirrors `SOLVELA_SIGNING_MODE`). */
export type SigningMode = 'auto' | 'escrow' | 'direct' | 'off';

export interface ResolveWalletOptions {
  /**
   * Base directory that holds `wallet.json`. Defaults to `~/.solvela`.
   * Overridable so tests can inject a tmpdir and never touch the real home dir.
   */
  baseDir?: string;
  /**
   * Current signing mode. In `off` mode no wallet is required and no file is
   * created (an explicit `SOLANA_WALLET_KEY` is still honored for display).
   */
  signingMode: SigningMode;
}

/** Where the resolved wallet came from. */
export type WalletSource = 'env' | 'file' | 'none';

export interface ResolvedWallet {
  /**
   * Base58 64-byte secret key, in the exact form `SOLANA_WALLET_KEY` expects.
   * `undefined` only when `source === 'none'` (off mode, no env key).
   * SECURITY: never log this value.
   */
  privateKey?: string;
  /** Base58 public address derived from the key. `undefined` only for `source === 'none'`. */
  address?: string;
  /** Resolution source. */
  source: WalletSource;
  /** True iff this call generated and wrote a brand-new wallet file. */
  created: boolean;
  /**
   * A human-readable, secret-free startup notice for stderr (never stdout —
   * stdout must stay clean for the stdio transport). Contains the address only.
   * `undefined` when there is nothing to say (e.g. env source with no notice).
   */
  notice?: string;
}

/** Number of bytes in a Solana secret key (`seed[0..32] || pubkey[32..64]`). */
const SECRET_KEY_BYTES = 64;

function defaultBaseDir(): string {
  return path.join(os.homedir(), '.solvela');
}

function walletFilePath(baseDir: string): string {
  return path.join(baseDir, 'wallet.json');
}

/**
 * Derive the Solana address from a base58 64-byte secret key, using the SAME
 * primitive the signer uses (`Keypair.fromSecretKey`). This both yields the
 * source-of-truth address and validates that the key is a well-formed,
 * self-consistent ed25519 secret key — `Keypair.fromSecretKey` rejects a key
 * whose pubkey half does not match the seed half.
 *
 * @throws Error with a redacted message (never echoes the key) on any decode or
 *         consistency failure.
 */
function deriveAddress(privateKeyB58: string): string {
  let decoded: Uint8Array;
  try {
    decoded = bs58.decode(privateKeyB58);
  } catch {
    // Do NOT include the key bytes/string in the message.
    throw new Error('private key is not valid base58');
  }
  if (decoded.length !== SECRET_KEY_BYTES) {
    throw new Error(
      `private key must decode to exactly ${SECRET_KEY_BYTES} bytes (got ${decoded.length})`,
    );
  }
  try {
    // Keypair.fromSecretKey verifies the seed→pubkey relationship and throws on
    // a tampered key. We never log the resulting secret.
    return Keypair.fromSecretKey(decoded).publicKey.toBase58();
  } catch (err) {
    // err.message from web3.js does not contain the key; still wrap defensively.
    throw new Error(
      `private key is not a valid ed25519 secret key: ${err instanceof Error ? err.message : 'unknown error'}`,
    );
  }
}

/**
 * Resolve (and, on first run, auto-create) the wallet.
 *
 * @throws Error on an invalid `SOLANA_WALLET_KEY`, a corrupt/tampered wallet
 *         file, or an I/O failure. Fails closed — never silently substitutes a
 *         different key or quotes a zero/empty wallet.
 */
export async function resolveWallet(opts: ResolveWalletOptions): Promise<ResolvedWallet> {
  const baseDir = opts.baseDir ?? defaultBaseDir();
  const filePath = walletFilePath(baseDir);

  // 1. Explicit env key wins. Do NOT read or create any file.
  const envKey = process.env['SOLANA_WALLET_KEY'];
  if (envKey !== undefined && envKey !== '') {
    let address: string;
    try {
      address = deriveAddress(envKey);
    } catch (err) {
      // Fail closed — never fall through to the file when an explicit key is set.
      throw new Error(
        `SOLANA_WALLET_KEY is set but invalid: ${err instanceof Error ? err.message : 'unknown error'}`,
      );
    }
    return {
      privateKey: envKey,
      address,
      source: 'env',
      created: false,
      // From env: a single quiet line, no key. Address only.
      notice: `[solvela-mcp] Using wallet from SOLANA_WALLET_KEY → ${address}`,
    };
  }

  // 2/3. File-backed resolution only happens when signing is actually needed.
  // In `off` mode require no wallet and create no file.
  if (opts.signingMode === 'off') {
    return { source: 'none', created: false };
  }

  // 2. Existing file → load + validate.
  if (await fileExists(filePath)) {
    const { privateKey, address } = await loadWalletFile(filePath);
    return {
      privateKey,
      address,
      source: 'file',
      created: false,
      notice: `[solvela-mcp] Loaded Solvela wallet → ${address}`,
    };
  }

  // 3. Neither → auto-generate + write.
  const { privateKey, address } = generateWallet();
  // createIfAbsent returns the actual on-disk wallet: if a concurrent run won
  // the race and wrote first, we adopt THAT wallet rather than clobbering it.
  const persisted = await createIfAbsent(baseDir, filePath, privateKey, address);

  if (persisted.created) {
    return {
      privateKey: persisted.privateKey,
      address: persisted.address,
      source: 'file',
      created: true,
      notice:
        `[solvela-mcp] Created a new Solvela wallet → ${persisted.address}. ` +
        `Fund it with USDC-SPL on Solana to start paying for calls — network gas is ` +
        `covered by the gateway's faucet, so no SOL needed. ` +
        `Back it up: ${filePath} controls your funds.`,
    };
  }
  // Lost the create race — adopt the winner's file.
  return {
    privateKey: persisted.privateKey,
    address: persisted.address,
    source: 'file',
    created: false,
    notice: `[solvela-mcp] Loaded Solvela wallet → ${persisted.address}`,
  };
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

async function fileExists(p: string): Promise<boolean> {
  try {
    await fs.access(p);
    return true;
  } catch {
    return false;
  }
}

interface WalletFields {
  privateKey: string;
  address: string;
}

/**
 * Load and validate a wallet.json. Fails closed on any inconsistency.
 *
 * Validation (per spec): the decoded `private_key` is exactly 64 bytes AND the
 * pubkey derived from `seed[0..32]` base58-encodes to the stored `address`. A
 * mismatch (corruption / tampering) throws rather than silently using the file.
 */
async function loadWalletFile(filePath: string): Promise<WalletFields> {
  const raw = await fs.readFile(filePath, 'utf-8');

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error(`wallet file ${filePath} is not valid JSON`);
  }
  if (typeof parsed !== 'object' || parsed === null) {
    throw new Error(`wallet file ${filePath} is not a JSON object`);
  }
  const obj = parsed as Record<string, unknown>;

  const privateKey = obj['private_key'];
  if (typeof privateKey !== 'string' || privateKey === '') {
    throw new Error(`wallet file ${filePath} is missing a string 'private_key' field`);
  }
  const storedAddress = obj['address'];
  if (typeof storedAddress !== 'string' || storedAddress === '') {
    throw new Error(`wallet file ${filePath} is missing a string 'address' field`);
  }

  // deriveAddress enforces the 64-byte length + ed25519 self-consistency and
  // gives us the canonical pubkey to compare against the stored address.
  const derived = deriveAddress(privateKey);
  if (derived !== storedAddress) {
    // Tampered / corrupt: the stored address does not match the key. Fail
    // closed — never sign with a key whose advertised address is wrong.
    throw new Error(
      `wallet file ${filePath} address does not match its private key ` +
        `(stored=${storedAddress}, derived=${derived}). Refusing to use a tampered wallet.`,
    );
  }

  return { privateKey, address: derived };
}

/** Generate a fresh ed25519 keypair in the Solana base58 64-byte secret-key form. */
function generateWallet(): WalletFields {
  // node:crypto ed25519 — no extra dependency, FIPS-grade CSPRNG. We extract the
  // raw 32-byte seed (tail of the PKCS8 DER) and raw 32-byte pubkey (tail of the
  // SPKI DER), then assemble the Solana `seed || pubkey` secret-key layout.
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const spki = publicKey.export({ type: 'spki', format: 'der' });
  const pkcs8 = privateKey.export({ type: 'pkcs8', format: 'der' });
  const rawPub = spki.subarray(spki.length - 32);
  const rawSeed = pkcs8.subarray(pkcs8.length - 32);

  const secret = Buffer.concat([rawSeed, rawPub]);
  const privateKeyB58 = bs58.encode(secret);
  // Derive via the signer's own primitive so the persisted address is exactly
  // what the signer will produce, and the key is validated before it is written.
  const address = deriveAddress(privateKeyB58);
  return { privateKey: privateKeyB58, address };
}

interface PersistResult {
  privateKey: string;
  address: string;
  /** True iff THIS call wrote the file; false iff a concurrent run won the race. */
  created: boolean;
}

/**
 * Atomically and exclusively persist a freshly-generated wallet, never
 * overwriting an existing wallet.json.
 *
 * Steps:
 *  1. Ensure `<baseDir>` exists with mode 0700 (chmod after mkdir — mkdir's mode
 *     is not applied to a pre-existing directory on Linux).
 *  2. Open a UNIQUELY-named temp file in the same dir with `wx` (O_EXCL) so a
 *     concurrent run cannot clobber or race our temp.
 *  3. fchmod 0600 on the FD, write, fsync, close — the file is never visible on
 *     disk with wider permissions.
 *  4. `link` the temp to the destination (atomic, fails with EEXIST if another
 *     run already created wallet.json), then unlink the temp. If the link
 *     fails because the destination already exists, adopt that existing wallet
 *     (re-read it) — we never clobber it.
 */
async function createIfAbsent(
  baseDir: string,
  filePath: string,
  privateKeyB58: string,
  address: string,
): Promise<PersistResult> {
  await fs.mkdir(baseDir, { recursive: true, mode: 0o700 });
  if (process.platform !== 'win32') {
    // mkdir({mode}) does not apply the mode to a pre-existing dir on Linux.
    await fs.chmod(baseDir, 0o700).catch(() => {});
  }

  const json = JSON.stringify(
    {
      private_key: privateKeyB58,
      address,
      created_at: new Date().toISOString(),
    },
    null,
    2,
  );

  // Unique temp name in the SAME directory (same filesystem → atomic link/rename).
  const tmpPath = path.join(baseDir, `.wallet.json.tmp-${process.pid}-${randomBytes(6).toString('hex')}`);

  // Open EXCLUSIVE ('wx' = O_CREAT | O_EXCL | O_WRONLY) at mode 0600.
  let handle: fs.FileHandle | undefined;
  try {
    handle = await fs.open(tmpPath, 'wx', 0o600);
    // Defense-in-depth: fchmod 0600 on the FD before writing, in case the umask
    // widened the create mode.
    if (process.platform !== 'win32') {
      await handle.chmod(0o600);
    }
    await handle.writeFile(json, 'utf-8');
    await handle.sync();
  } finally {
    await handle?.close();
  }

  try {
    // link() is atomic and fails with EEXIST if wallet.json already exists —
    // this is the no-overwrite guarantee, race-safe across concurrent runs.
    await fs.link(tmpPath, filePath);
    await fs.unlink(tmpPath).catch(() => {});
    if (process.platform !== 'win32') {
      // Belt-and-braces: re-assert 0600 on the destination.
      await fs.chmod(filePath, 0o600).catch(() => {});
    }
    return { privateKey: privateKeyB58, address, created: true };
  } catch (err) {
    // Clean up our temp regardless of why link failed.
    await fs.unlink(tmpPath).catch(() => {});
    if ((err as NodeJS.ErrnoException).code === 'EEXIST') {
      // Another run won the race (or the file appeared between our existence
      // check and here). Adopt the existing wallet — never clobber it.
      const existing = await loadWalletFile(filePath);
      return { privateKey: existing.privateKey, address: existing.address, created: false };
    }
    throw err;
  }
}
