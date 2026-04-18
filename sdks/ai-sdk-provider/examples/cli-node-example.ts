/**
 * Node.js CLI example using createLocalWalletAdapter.
 *
 * DEV/TEST ONLY — the local adapter signs with an unencrypted keypair.
 * For production, implement SolvelaWalletAdapter with your infrastructure.
 *
 * Usage:
 *   export SOLANA_WALLET_KEY=<base58-encoded-keypair>
 *   npx tsx examples/cli-node-example.ts
 */

import { generateText } from 'ai';
import { createSolvelaProvider } from '@solvela/ai-sdk-provider';
import { createLocalWalletAdapter } from '@solvela/ai-sdk-provider/adapters/local';
import { Keypair } from '@solana/web3.js';
import bs58 from 'bs58';

async function main() {
  const walletKey = process.env.SOLANA_WALLET_KEY;
  if (!walletKey) {
    throw new Error('Set SOLANA_WALLET_KEY env var (base58-encoded keypair)');
  }

  const keypair = Keypair.fromSecretKey(bs58.decode(walletKey));
  const solvela = createSolvelaProvider({
    wallet: createLocalWalletAdapter(keypair),
  });

  const { text } = await generateText({
    model: solvela('anthropic-claude-sonnet-4-5'),
    prompt: 'Explain proof of history in one sentence.',
  });

  console.log('Response:', text);
}

main().catch(console.error);
