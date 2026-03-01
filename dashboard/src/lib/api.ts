import type { HealthResponse, PricingResponse } from "@/types";

const GATEWAY_URL =
  process.env.NEXT_PUBLIC_GATEWAY_URL ?? "http://localhost:8402";

export async function fetchHealth(): Promise<HealthResponse> {
  const res = await fetch(`${GATEWAY_URL}/health`, { cache: "no-store" });
  if (!res.ok) throw new Error(`Health check failed: ${res.status}`);
  return res.json();
}

export async function fetchPricing(): Promise<PricingResponse> {
  const res = await fetch(`${GATEWAY_URL}/pricing`, {
    next: { revalidate: 60 },
  });
  if (!res.ok) throw new Error(`Pricing fetch failed: ${res.status}`);
  return res.json();
}

export async function fetchModels(): Promise<{ data: PricingResponse["models"] }> {
  const res = await fetch(`${GATEWAY_URL}/v1/models`, {
    next: { revalidate: 60 },
  });
  if (!res.ok) throw new Error(`Models fetch failed: ${res.status}`);
  return res.json();
}
