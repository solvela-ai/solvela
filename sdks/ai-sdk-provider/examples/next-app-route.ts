/**
 * Next.js 16 app router example with custom SolvelaWalletAdapter.
 * This example shows the production adapter pattern.
 */

import { generateText } from 'ai';
import { createSolvelaProvider } from '@solvela/ai-sdk-provider';
import type { SolvelaWalletAdapter, SolvelaPaymentRequired } from '@solvela/ai-sdk-provider';

/**
 * Custom production adapter for Next.js app routes.
 * Integrates with your signing infrastructure (hardware wallet, MPC, etc.).
 */
class NextAppWalletAdapter implements SolvelaWalletAdapter {
  readonly label = 'next-app-signer';

  async signPayment(args: {
    paymentRequired: SolvelaPaymentRequired;
    resourceUrl: string;
    requestBody: string;
    signal?: AbortSignal;
  }): Promise<string> {
    // TODO: sign the payment with your infrastructure.
    // This example shows the interface — implement with your signer.
    //
    // 1. Extract USDC amount from args.paymentRequired.accepts[0].amount
    // 2. Build a Solana USDC-SPL transaction to args.paymentRequired.accepts[0].pay_to
    // 3. Sign with your wallet (hardware wallet, MPC service, etc.)
    // 4. Return base64-encoded PAYMENT-SIGNATURE header value

    throw new Error('Implement with your signing infrastructure');
  }
}

export async function POST(req: Request) {
  const adapter = new NextAppWalletAdapter();
  const solvela = createSolvelaProvider({
    baseURL: process.env.SOLVELA_API_URL || 'https://api.solvela.ai/v1',
    wallet: adapter,
    sessionBudget: BigInt(50_000_000), // $50 USDC in atomic units
  });

  const { text } = await generateText({
    model: solvela('anthropic-claude-sonnet-4-5'),
    prompt: 'What is Solana?',
  });

  return Response.json({ reply: text });
}
