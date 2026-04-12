import type { Plugin } from "@elizaos/core";
import { chatViaRustyClaw } from "./actions/chat.js";
import { gatewayProvider } from "./providers/gateway.js";

export const rustyClawPlugin: Plugin = {
  name: "rustyclaw",
  description:
    "Solvela integration — Solana-native AI agent payments via x402",
  actions: [chatViaRustyClaw],
  providers: [gatewayProvider],
};

export default rustyClawPlugin;
