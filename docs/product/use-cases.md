# Use Cases

## Autonomous AI Agents

AI agents that run continuously -- monitoring markets, managing infrastructure, conducting research -- need to call LLM APIs without waiting for a human to approve each charge. Credit cards require a human account holder. API keys require a managed account and leak risk. With Solvela, the agent just needs a funded Solana wallet. It pays per call, autonomously, with cryptographic proof of every transaction. No accounts, no approvals, no billing disputes.

## Multi-Model Routing

Different questions need different models. A simple classification task doesn't need the same model as a complex reasoning problem. Solvela's smart router analyzes each request across 15 dimensions and selects the best model from five providers (OpenAI, Anthropic, Google, xAI, DeepSeek). The agent sends one request to one endpoint, pays one price, and gets the optimal response. No need to manage multiple API keys or learn each provider's pricing.

## Pay-Per-Call Billing

No monthly subscriptions. No upfront commitments. No minimum spend. Each API call is priced individually based on the model used and tokens consumed. An agent that makes one call per day pays for one call per day. An agent that makes a million calls pays for a million calls. Every response includes a cost breakdown: provider cost, platform fee (5%), and total in USDC.

## Trustless Escrow

For expensive operations -- long multi-turn conversations, batch processing, or tasks where the final cost is uncertain -- agents can deposit into an on-chain escrow. The smart contract holds the maximum estimated cost and releases only the actual amount after service delivery. The remainder refunds automatically. If the gateway never claims (server failure, network issue), the agent reclaims the full deposit after a timeout. No trust in the gateway required. The math is enforced by code on the blockchain.

## Enterprise Team Management

Organizations running multiple AI agents can create teams, assign Solana wallets, and set spend limits -- per team and per hour. API key authentication lets agents authenticate programmatically without wallet signatures on every request. Full audit trails track every action: who made which request, which model was used, how much it cost. Budget controls prevent runaway spending before it happens, not after.
