/**
 * Tests for wallet resolution + auto-creation (src/wallet.ts).
 *
 * Covers the new `resolveWallet()` precedence chain:
 *   1. SOLANA_WALLET_KEY env  (explicit, wins; no file read or created)
 *   2. ~/.solvela/wallet.json `private_key`  (shared with the Rust CLI)
 *   3. neither → auto-generate ed25519 + write ~/.solvela/wallet.json
 *
 * SECURITY: every test uses an injected tmpdir base — it MUST never touch the
 * real ~/.solvela. The private key is a secret; assertions verify it is never
 * present in any log/notice string and that file perms are 0600 / dir 0700.
 *
 * Harness: Node's built-in test runner (node --test), matching the rest of the
 * MCP test suite — NOT Vitest. TS is type-stripped by Node 22.
 */

import { describe, it, before, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';

import { Keypair } from '@solana/web3.js';
import bs58 from 'bs58';

import { resolveWallet } from '../src/wallet.ts';

// ---------------------------------------------------------------------------
// Known golden keypair vector (fixed seed 0x01..0x20).
// Verified to round-trip through @solana/web3.js Keypair.fromSecretKey:
//   private_key = base58(seed[0..32] || pubkey[32..64])
//   address     = base58(pubkey)
// Reproduce: node script in the auto-wallet implementation notes.
// ---------------------------------------------------------------------------
const GOLDEN = {
  private_key:
    '2Ana1pUpv2ZbMVkwF5FXapYeBEjdxDatLn7nvJkhgTSdZd8hbDHTd21as7EAsg7ypityqfsw2pMQKJcVDVcAEsd',
  address: '9C6hybhQ6Aycep9jaUnP6uL9ZYvDjUp1aSkFWPUFJtpj',
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async function makeTempBase(): Promise<string> {
  // The base dir stands in for ~/.solvela — resolveWallet writes wallet.json here.
  return fs.mkdtemp(path.join(os.tmpdir(), 'solvela-wallet-test-'));
}

async function cleanupDir(dir: string): Promise<void> {
  await fs.rm(dir, { recursive: true, force: true });
}

function walletPath(base: string): string {
  return path.join(base, 'wallet.json');
}

async function writeWalletFile(base: string, contents: unknown): Promise<void> {
  await fs.mkdir(base, { recursive: true });
  await fs.writeFile(walletPath(base), JSON.stringify(contents, null, 2), 'utf-8');
}

/** Pubkey derived from the stored private key, via the SAME primitive the signer uses. */
function derivedAddress(privateKeyB58: string): string {
  return Keypair.fromSecretKey(bs58.decode(privateKeyB58)).publicKey.toBase58();
}

before(() => {
  delete process.env['SOLANA_WALLET_KEY'];
  delete process.env['SOLANA_WALLET_ADDRESS'];
  delete process.env['SOLVELA_SIGNING_MODE'];
});

// Each test owns its env; clear the wallet key between tests so leakage from one
// test can never satisfy another's precedence assertions.
beforeEach(() => {
  delete process.env['SOLANA_WALLET_KEY'];
});

afterEach(() => {
  delete process.env['SOLANA_WALLET_KEY'];
});

// ---------------------------------------------------------------------------
// 1. Parity golden — CLI-schema file loads, and round-trips back out.
// ---------------------------------------------------------------------------

describe('resolveWallet — golden parity with the Rust CLI schema', () => {
  it('loads a wallet.json written in the CLI schema → correct address + usable signer', async () => {
    const base = await makeTempBase();
    try {
      await writeWalletFile(base, {
        private_key: GOLDEN.private_key,
        address: GOLDEN.address,
        created_at: '2026-01-01T00:00:00Z',
      });

      const resolved = await resolveWallet({ baseDir: base, signingMode: 'auto' });

      assert.equal(resolved.address, GOLDEN.address, 'address must match stored vector');
      assert.equal(resolved.privateKey, GOLDEN.private_key, 'private key must be returned verbatim');
      assert.equal(resolved.source, 'file');
      assert.equal(resolved.created, false);

      // The resolved key must plug into the signer's decoding identically.
      const signerAddress = derivedAddress(resolved.privateKey);
      assert.equal(signerAddress, GOLDEN.address, 'signer-derived pubkey must equal stored address');
    } finally {
      await cleanupDir(base);
    }
  });

  it('a file written by resolveWallet round-trips: re-load yields the same key + address', async () => {
    const base = await makeTempBase();
    try {
      const first = await resolveWallet({ baseDir: base, signingMode: 'auto' });
      assert.equal(first.created, true, 'first run must auto-create');

      const second = await resolveWallet({ baseDir: base, signingMode: 'auto' });
      assert.equal(second.source, 'file', 'second run must load the file');
      assert.equal(second.created, false, 'second run must NOT re-create');
      assert.equal(second.privateKey, first.privateKey, 'private key must survive round-trip');
      assert.equal(second.address, first.address, 'address must survive round-trip');
    } finally {
      await cleanupDir(base);
    }
  });

  it('the on-disk schema matches the CLI: private_key = base58(seed || pubkey), address = base58(pubkey)', async () => {
    const base = await makeTempBase();
    try {
      const resolved = await resolveWallet({ baseDir: base, signingMode: 'auto' });

      const raw = JSON.parse(await fs.readFile(walletPath(base), 'utf-8')) as {
        private_key: string;
        address: string;
        created_at: string;
      };

      // 64-byte secret-key convention.
      const secret = bs58.decode(raw.private_key);
      assert.equal(secret.length, 64, 'private_key must decode to exactly 64 bytes');

      // address must be base58(pubkey == bytes[32..64]).
      assert.equal(bs58.encode(secret.subarray(32)), raw.address, 'address must be base58 of pubkey half');
      // And the pubkey half must be the ed25519 pubkey of the seed half — verified
      // through the signer's Keypair.fromSecretKey (rejects an inconsistent key).
      assert.equal(derivedAddress(raw.private_key), raw.address, 'pubkey must derive from seed half');

      // created_at must be a parseable RFC3339 timestamp.
      assert.ok(!Number.isNaN(Date.parse(raw.created_at)), 'created_at must be a valid timestamp');
    } finally {
      await cleanupDir(base);
    }
  });
});

// ---------------------------------------------------------------------------
// 2. Env precedence — SOLANA_WALLET_KEY wins; no file read or created.
// ---------------------------------------------------------------------------

describe('resolveWallet — SOLANA_WALLET_KEY env precedence', () => {
  it('uses the env key and never reads or creates a file', async () => {
    const base = await makeTempBase();
    try {
      process.env['SOLANA_WALLET_KEY'] = GOLDEN.private_key;

      const resolved = await resolveWallet({ baseDir: base, signingMode: 'auto' });

      assert.equal(resolved.source, 'env');
      assert.equal(resolved.created, false);
      assert.equal(resolved.address, GOLDEN.address, 'address must be derived from the env key');
      assert.equal(resolved.privateKey, GOLDEN.private_key);

      // No file should have been created.
      await assert.rejects(
        fs.access(walletPath(base)),
        'no wallet.json may be created when SOLANA_WALLET_KEY is set',
      );
    } finally {
      await cleanupDir(base);
    }
  });

  it('env wins even when a file is also present (file is not read)', async () => {
    const base = await makeTempBase();
    try {
      // A DIFFERENT key on disk. If the file were read, the address would differ.
      const fileKp = Keypair.generate();
      await writeWalletFile(base, {
        private_key: bs58.encode(fileKp.secretKey),
        address: fileKp.publicKey.toBase58(),
        created_at: '2026-01-01T00:00:00Z',
      });

      process.env['SOLANA_WALLET_KEY'] = GOLDEN.private_key;
      const resolved = await resolveWallet({ baseDir: base, signingMode: 'auto' });

      assert.equal(resolved.source, 'env');
      assert.equal(resolved.address, GOLDEN.address, 'must use env key, not the on-disk key');
      assert.notEqual(resolved.address, fileKp.publicKey.toBase58());
    } finally {
      await cleanupDir(base);
    }
  });

  it('rejects an invalid SOLANA_WALLET_KEY rather than silently falling back to a file', async () => {
    const base = await makeTempBase();
    try {
      await writeWalletFile(base, {
        private_key: GOLDEN.private_key,
        address: GOLDEN.address,
        created_at: '2026-01-01T00:00:00Z',
      });
      process.env['SOLANA_WALLET_KEY'] = 'not-a-valid-base58-key!!!';

      await assert.rejects(
        resolveWallet({ baseDir: base, signingMode: 'auto' }),
        /SOLANA_WALLET_KEY/,
        'an invalid env key must throw, not fall through to the file',
      );
    } finally {
      await cleanupDir(base);
    }
  });
});

// ---------------------------------------------------------------------------
// 3. File fallback — no env, file present → loaded.
// ---------------------------------------------------------------------------

describe('resolveWallet — file fallback', () => {
  it('loads the existing wallet.json when no env key is set', async () => {
    const base = await makeTempBase();
    try {
      await writeWalletFile(base, {
        private_key: GOLDEN.private_key,
        address: GOLDEN.address,
        created_at: '2026-01-01T00:00:00Z',
      });

      const resolved = await resolveWallet({ baseDir: base, signingMode: 'auto' });
      assert.equal(resolved.source, 'file');
      assert.equal(resolved.created, false);
      assert.equal(resolved.address, GOLDEN.address);
      assert.equal(resolved.privateKey, GOLDEN.private_key);
    } finally {
      await cleanupDir(base);
    }
  });
});

// ---------------------------------------------------------------------------
// 4. Auto-create — no env, no file → file created with correct schema.
// ---------------------------------------------------------------------------

describe('resolveWallet — auto-create on first run', () => {
  it('creates wallet.json with a valid keypair and derived address', async () => {
    const base = await makeTempBase();
    try {
      const resolved = await resolveWallet({ baseDir: base, signingMode: 'auto' });

      assert.equal(resolved.source, 'file');
      assert.equal(resolved.created, true, 'first run must report created=true');

      // File exists and parses.
      const raw = JSON.parse(await fs.readFile(walletPath(base), 'utf-8')) as {
        private_key: string;
        address: string;
        created_at: string;
      };

      // Schema fields present.
      assert.ok(raw.private_key && raw.address && raw.created_at, 'all schema fields present');

      // Keypair is valid and self-consistent.
      const secret = bs58.decode(raw.private_key);
      assert.equal(secret.length, 64);
      assert.equal(derivedAddress(raw.private_key), raw.address, 'address derives from key');

      // resolveWallet's returned values match the file.
      assert.equal(resolved.address, raw.address);
      assert.equal(resolved.privateKey, raw.private_key);
    } finally {
      await cleanupDir(base);
    }
  });

  it('creates the base dir if it does not exist', async () => {
    const parent = await makeTempBase();
    try {
      const base = path.join(parent, 'nested', '.solvela');
      const resolved = await resolveWallet({ baseDir: base, signingMode: 'auto' });
      assert.equal(resolved.created, true);
      await fs.access(walletPath(base)); // throws if missing
    } finally {
      await cleanupDir(parent);
    }
  });
});

// ---------------------------------------------------------------------------
// 5. Permissions — file 0600, dir 0700.
// ---------------------------------------------------------------------------

describe('resolveWallet — file/dir permissions', () => {
  it('created wallet.json is mode 0600 and the dir is 0700', async (t) => {
    if (process.platform === 'win32') {
      t.skip('POSIX permission semantics do not apply on Windows');
      return;
    }
    const base = await makeTempBase();
    try {
      await resolveWallet({ baseDir: base, signingMode: 'auto' });

      const fileStat = await fs.stat(walletPath(base));
      assert.equal(fileStat.mode & 0o777, 0o600, 'wallet.json must be 0600');

      const dirStat = await fs.stat(base);
      assert.equal(dirStat.mode & 0o777, 0o700, '.solvela dir must be 0700');
    } finally {
      await cleanupDir(base);
    }
  });
});

// ---------------------------------------------------------------------------
// 6. No-overwrite — a second run does not clobber the existing file.
// ---------------------------------------------------------------------------

describe('resolveWallet — never overwrites an existing wallet', () => {
  it('a second resolveWallet keeps the original key bytes', async () => {
    const base = await makeTempBase();
    try {
      const first = await resolveWallet({ baseDir: base, signingMode: 'auto' });
      const bytesBefore = await fs.readFile(walletPath(base), 'utf-8');

      const second = await resolveWallet({ baseDir: base, signingMode: 'auto' });
      const bytesAfter = await fs.readFile(walletPath(base), 'utf-8');

      assert.equal(second.created, false);
      assert.equal(second.privateKey, first.privateKey);
      assert.equal(bytesAfter, bytesBefore, 'file contents must be byte-identical after a second run');
    } finally {
      await cleanupDir(base);
    }
  });
});

// ---------------------------------------------------------------------------
// 7. Validation — corrupt/tampered file throws, never silently used.
// ---------------------------------------------------------------------------

describe('resolveWallet — validation of an on-disk file (fail closed)', () => {
  it('throws on a wrong-length private_key', async () => {
    const base = await makeTempBase();
    try {
      // 32-byte key (seed only) — not the 64-byte secret-key form.
      const shortKey = bs58.encode(Buffer.alloc(32, 7));
      await writeWalletFile(base, {
        private_key: shortKey,
        address: GOLDEN.address,
        created_at: '2026-01-01T00:00:00Z',
      });

      await assert.rejects(
        resolveWallet({ baseDir: base, signingMode: 'auto' }),
        /64 bytes|length/i,
        'a non-64-byte key must be rejected, not used',
      );
    } finally {
      await cleanupDir(base);
    }
  });

  it('throws when the stored address does not match the key (tampered file)', async () => {
    const base = await makeTempBase();
    try {
      // Valid 64-byte key, but the stored address belongs to a DIFFERENT key.
      const otherAddress = Keypair.generate().publicKey.toBase58();
      await writeWalletFile(base, {
        private_key: GOLDEN.private_key,
        address: otherAddress,
        created_at: '2026-01-01T00:00:00Z',
      });

      await assert.rejects(
        resolveWallet({ baseDir: base, signingMode: 'auto' }),
        /address|mismatch/i,
        'a key/address mismatch must be rejected, not silently used',
      );
    } finally {
      await cleanupDir(base);
    }
  });

  it('throws on a missing private_key field', async () => {
    const base = await makeTempBase();
    try {
      await writeWalletFile(base, { address: GOLDEN.address, created_at: '2026-01-01T00:00:00Z' });
      await assert.rejects(
        resolveWallet({ baseDir: base, signingMode: 'auto' }),
        /private_key/,
      );
    } finally {
      await cleanupDir(base);
    }
  });

  it('throws on a non-JSON / corrupt file', async () => {
    const base = await makeTempBase();
    try {
      await fs.mkdir(base, { recursive: true });
      await fs.writeFile(walletPath(base), 'not json at all', 'utf-8');
      await assert.rejects(resolveWallet({ baseDir: base, signingMode: 'auto' }));
    } finally {
      await cleanupDir(base);
    }
  });
});

// ---------------------------------------------------------------------------
// 8. Signing off — no wallet required, no file created.
// ---------------------------------------------------------------------------

describe('resolveWallet — signingMode=off', () => {
  it('requires no wallet and creates no file', async () => {
    const base = await makeTempBase();
    try {
      const resolved = await resolveWallet({ baseDir: base, signingMode: 'off' });

      assert.equal(resolved.source, 'none');
      assert.equal(resolved.created, false);
      assert.equal(resolved.privateKey, undefined, 'no key in off mode');
      assert.equal(resolved.address, undefined, 'no address in off mode');

      await assert.rejects(
        fs.access(walletPath(base)),
        'off mode must not create a wallet file',
      );
    } finally {
      await cleanupDir(base);
    }
  });

  it('off mode still honors an explicit env key without creating a file', async () => {
    // off mode does not sign, but if the user explicitly set a key we surface
    // its address; we still must not create a file.
    const base = await makeTempBase();
    try {
      process.env['SOLANA_WALLET_KEY'] = GOLDEN.private_key;
      const resolved = await resolveWallet({ baseDir: base, signingMode: 'off' });
      assert.equal(resolved.source, 'env');
      assert.equal(resolved.address, GOLDEN.address);
      await assert.rejects(fs.access(walletPath(base)));
    } finally {
      await cleanupDir(base);
    }
  });
});

// ---------------------------------------------------------------------------
// 9. Redaction — notice/log strings contain the address, NEVER the private key.
// ---------------------------------------------------------------------------

describe('resolveWallet — secret redaction in notices', () => {
  it('the created-wallet notice contains the address but never the private key', async () => {
    const base = await makeTempBase();
    try {
      const resolved = await resolveWallet({ baseDir: base, signingMode: 'auto' });
      assert.ok(resolved.notice, 'a startup notice should be produced on create');
      assert.ok(resolved.notice!.includes(resolved.address!), 'notice should include the address');
      assert.ok(
        !resolved.notice!.includes(resolved.privateKey!),
        'notice must NEVER include the private key',
      );
    } finally {
      await cleanupDir(base);
    }
  });

  it('the loaded-wallet notice contains the address but never the private key', async () => {
    const base = await makeTempBase();
    try {
      await writeWalletFile(base, {
        private_key: GOLDEN.private_key,
        address: GOLDEN.address,
        created_at: '2026-01-01T00:00:00Z',
      });
      const resolved = await resolveWallet({ baseDir: base, signingMode: 'auto' });
      assert.ok(resolved.notice, 'a notice should be produced on load');
      assert.ok(resolved.notice!.includes(GOLDEN.address));
      assert.ok(!resolved.notice!.includes(GOLDEN.private_key), 'notice must not leak the key');
    } finally {
      await cleanupDir(base);
    }
  });

  it('the from-env path does not emit the private key in its notice', async () => {
    const base = await makeTempBase();
    try {
      process.env['SOLANA_WALLET_KEY'] = GOLDEN.private_key;
      const resolved = await resolveWallet({ baseDir: base, signingMode: 'auto' });
      if (resolved.notice) {
        assert.ok(!resolved.notice.includes(GOLDEN.private_key), 'env notice must not leak the key');
      }
    } finally {
      await cleanupDir(base);
    }
  });
});
