import type { Provider, IAgentRuntime, Memory } from "@elizaos/core";

export const gatewayProvider: Provider = {
  get: async (runtime: IAgentRuntime, _message: Memory) => {
    const gatewayUrl =
      runtime.getSetting("RUSTYCLAW_GATEWAY_URL") || "http://localhost:8402";

    try {
      const resp = await fetch(`${gatewayUrl}/health`);
      if (!resp.ok) {
        return `Solvela gateway at ${gatewayUrl} returned ${resp.status}.`;
      }
      const health = await resp.json();
      return `Solvela gateway at ${gatewayUrl} is ${health.status || "online"}.`;
    } catch (err) {
      return `Solvela gateway at ${gatewayUrl} is unreachable: ${err}`;
    }
  },
};
