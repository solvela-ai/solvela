import type { Plugin } from "@elizaos/core";
import { chatViaSolvela } from "./actions/chat.js";
import { gatewayProvider } from "./providers/gateway.js";

export const solvelaPlugin: Plugin = {
  name: "solvela",
  description:
    "Solvela integration — Solana-native AI agent payments via x402",
  actions: [chatViaSolvela],
  providers: [gatewayProvider],
};

export default solvelaPlugin;
