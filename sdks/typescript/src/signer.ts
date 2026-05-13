import { Connection, PublicKey, Transaction } from '@solana/web3.js';
import { getAssociatedTokenAddress, createTransferInstruction } from '@solana/spl-token';

import { PaymentAccept, PaymentPayload, Resource, SolanaPayload } from './types.js';
import { SignerError } from './errors.js';
import { Wallet } from './wallet.js';
import { USDC_MINT, X402_VERSION } from './constants.js';

export interface Signer {
  signPayment(
    amountAtomic: number,
    recipient: string,
    resource: Resource,
    accepted: PaymentAccept,
  ): Promise<PaymentPayload>;
}

export class KeypairSigner implements Signer {
  constructor(
    private readonly wallet: Wallet,
    private readonly rpcUrl: string = 'https://api.mainnet-beta.solana.com',
  ) {}

  async signPayment(
    amountAtomic: number,
    recipient: string,
    resource: Resource,
    accepted: PaymentAccept,
  ): Promise<PaymentPayload> {
    try {
      const connection = new Connection(this.rpcUrl);
      const mint = new PublicKey(USDC_MINT);
      const sender = this.wallet.publicKey();
      const recipientPubkey = new PublicKey(recipient);

      const senderAta = await getAssociatedTokenAddress(mint, sender);
      const recipientAta = await getAssociatedTokenAddress(mint, recipientPubkey);

      const ix = createTransferInstruction(senderAta, recipientAta, sender, amountAtomic);
      const { blockhash } = await connection.getLatestBlockhash('finalized');

      const tx = new Transaction({ recentBlockhash: blockhash, feePayer: sender });
      tx.add(ix);
      this.wallet.signTransaction(tx);

      const txB64 = tx.serialize().toString('base64');

      return new PaymentPayload(
        X402_VERSION,
        resource,
        accepted,
        new SolanaPayload(txB64),
      );
    } catch (e) {
      if (e instanceof SignerError) throw e;
      throw new SignerError(`Failed to sign payment: ${(e as Error).message}`);
    }
  }
}
