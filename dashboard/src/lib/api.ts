import type {
  HealthResponse,
  PricingResponse,
  AdminStatsResponse,
  ServicesResponse,
  EscrowConfig,
  EscrowHealth,
} from "@/types";

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

/**
 * Fetch admin stats from the gateway. Server-side only — reads GATEWAY_ADMIN_KEY
 * from process.env (never exposed to the client).
 * Returns null on failure so pages can fall back to mock data gracefully.
 */
export async function fetchAdminStats(
  days: number = 30,
): Promise<AdminStatsResponse | null> {
  const adminKey = process.env.GATEWAY_ADMIN_KEY;
  if (!adminKey) {
    console.warn("[fetchAdminStats] GATEWAY_ADMIN_KEY not set — skipping fetch");
    return null;
  }

  try {
    const res = await fetch(
      `${GATEWAY_URL}/v1/admin/stats?days=${days}`,
      {
        headers: { Authorization: `Bearer ${adminKey}` },
        next: { revalidate: 60 },
      },
    );

    if (!res.ok) {
      console.error(`[fetchAdminStats] HTTP ${res.status} from ${GATEWAY_URL}/v1/admin/stats`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.error("[fetchAdminStats] Failed to reach gateway:", error);
    return null;
  }
}

/**
 * Fetch the services marketplace list. Public endpoint, no auth needed.
 * Returns null on failure.
 */
export async function fetchServices(): Promise<ServicesResponse | null> {
  try {
    const res = await fetch(`${GATEWAY_URL}/v1/services`, {
      next: { revalidate: 60 },
    });

    if (!res.ok) {
      console.error(`[fetchServices] HTTP ${res.status} from ${GATEWAY_URL}/v1/services`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.error("[fetchServices] Failed to reach gateway:", error);
    return null;
  }
}

/**
 * Fetch escrow configuration. Public endpoint, no auth needed.
 * Returns null on failure.
 */
export async function fetchEscrowConfig(): Promise<EscrowConfig | null> {
  try {
    const res = await fetch(`${GATEWAY_URL}/v1/escrow/config`, {
      next: { revalidate: 60 },
    });

    if (!res.ok) {
      console.error(`[fetchEscrowConfig] HTTP ${res.status} from ${GATEWAY_URL}/v1/escrow/config`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.error("[fetchEscrowConfig] Failed to reach gateway:", error);
    return null;
  }
}

/**
 * Fetch escrow health. Admin endpoint — requires GATEWAY_ADMIN_KEY.
 * Returns null on failure.
 */
export async function fetchEscrowHealth(): Promise<EscrowHealth | null> {
  const adminKey = process.env.GATEWAY_ADMIN_KEY;
  if (!adminKey) {
    console.warn("[fetchEscrowHealth] GATEWAY_ADMIN_KEY not set — skipping fetch");
    return null;
  }

  try {
    const res = await fetch(`${GATEWAY_URL}/v1/escrow/health`, {
      headers: { Authorization: `Bearer ${adminKey}` },
      next: { revalidate: 60 },
    });

    if (!res.ok) {
      console.error(`[fetchEscrowHealth] HTTP ${res.status} from ${GATEWAY_URL}/v1/escrow/health`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.error("[fetchEscrowHealth] Failed to reach gateway:", error);
    return null;
  }
}
