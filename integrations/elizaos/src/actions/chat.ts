import type {
  Action,
  IAgentRuntime,
  Memory,
  HandlerCallback,
  State,
} from "@elizaos/core";

export const chatViaRustyClaw: Action = {
  name: "CHAT_VIA_RUSTYCLAW",
  description:
    "Send a chat completion through Solvela with Solana x402 payment",
  similes: ["llm call", "ai inference", "model query", "ask ai"],

  validate: async (runtime: IAgentRuntime) => {
    return !!runtime.getSetting("RUSTYCLAW_GATEWAY_URL");
  },

  handler: async (
    runtime: IAgentRuntime,
    message: Memory,
    _state: State | undefined,
    _options: Record<string, unknown>,
    callback: HandlerCallback,
  ) => {
    const gatewayUrl = runtime.getSetting("RUSTYCLAW_GATEWAY_URL");
    const model =
      runtime.getSetting("RUSTYCLAW_DEFAULT_MODEL") || "auto";

    const reqBody = {
      model,
      messages: [{ role: "user", content: message.content.text }],
    };

    try {
      // Step 1: Send request (may get 402 or 200 for free models)
      const resp = await fetch(`${gatewayUrl}/v1/chat/completions`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(reqBody),
      });

      if (resp.status === 402) {
        // Payment required — need wallet integration
        const paymentInfo = await resp.json();
        callback({
          text: `Payment required: ${paymentInfo.cost_breakdown?.total || "unknown"} USDC. Wallet signing not yet implemented in this plugin.`,
        });
        return false;
      }

      if (resp.ok) {
        const result = await resp.json();
        const content =
          result.choices?.[0]?.message?.content || "No response received.";
        callback({ text: content });
        return true;
      }

      callback({
        text: `Gateway error: ${resp.status} ${resp.statusText}`,
      });
      return false;
    } catch (err) {
      callback({ text: `Failed to reach gateway: ${err}` });
      return false;
    }
  },

  examples: [
    [
      {
        user: "{{user1}}",
        content: { text: "Ask the AI to explain quicksort" },
      },
      {
        user: "{{agentName}}",
        content: {
          text: "I'll query Solvela for that.",
          action: "CHAT_VIA_RUSTYCLAW",
        },
      },
    ],
  ],
};
