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

// ─── Auth header helper ───────────────────────────────────────────────────────

function getAuthHeaders(): HeadersInit {
  if (typeof window !== "undefined") {
    const apiKey = localStorage.getItem("rcr_api_key");
    if (apiKey) return { Authorization: `Bearer ${apiKey}` };
  }
  const adminKey = process.env.GATEWAY_ADMIN_KEY;
  if (adminKey) return { Authorization: `Bearer ${adminKey}` };
  return {};
}

// ─── Org/team types ───────────────────────────────────────────────────────────

export interface OrgEntry {
  id: string;
  name: string;
  slug: string;
  created_at: string;
  [key: string]: unknown;
}

export interface TeamEntry {
  id: string;
  org_id: string;
  name: string;
  wallet_count?: number;
  budget?: TeamBudget | null;
  created_at: string;
  [key: string]: unknown;
}

export interface MemberEntry {
  id: string;
  org_id: string;
  wallet_address: string;
  role: string;
  created_at: string;
  [key: string]: unknown;
}

export interface ApiKeyEntry {
  id: string;
  org_id: string;
  name: string;
  role: string;
  key_prefix: string;
  created_at: string;
  [key: string]: unknown;
}

export interface CreateApiKeyResponse {
  id: string;
  key: string;
  name: string;
  role: string;
  created_at: string;
  [key: string]: unknown;
}

export interface AuditLogEntry {
  id: string;
  org_id: string;
  action: string;
  resource_type: string;
  resource_id?: string;
  actor_key_id?: string;
  created_at: string;
  [key: string]: unknown;
}

export interface TeamBudget {
  team_id: string;
  daily_usdc?: number | null;
  monthly_usdc?: number | null;
  [key: string]: unknown;
}

export interface OrgStats {
  org_id: string;
  period_days: number;
  total_requests: number;
  total_cost_usdc: string;
  [key: string]: unknown;
}

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

// ─── Org / team / audit API functions ────────────────────────────────────────

export async function fetchOrgs(): Promise<OrgEntry[] | null> {
  try {
    const res = await fetch(`${GATEWAY_URL}/v1/orgs`, {
      headers: getAuthHeaders(),
      cache: "no-store",
    });
    if (!res.ok) {
      console.warn(`[fetchOrgs] HTTP ${res.status}`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.warn("[fetchOrgs] Failed to reach gateway:", error);
    return null;
  }
}

export async function fetchTeams(orgId: string): Promise<TeamEntry[] | null> {
  try {
    const res = await fetch(`${GATEWAY_URL}/v1/orgs/${orgId}/teams`, {
      headers: getAuthHeaders(),
      cache: "no-store",
    });
    if (!res.ok) {
      console.warn(`[fetchTeams] HTTP ${res.status}`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.warn("[fetchTeams] Failed to reach gateway:", error);
    return null;
  }
}

export async function fetchMembers(orgId: string): Promise<MemberEntry[] | null> {
  try {
    const res = await fetch(`${GATEWAY_URL}/v1/orgs/${orgId}/members`, {
      headers: getAuthHeaders(),
      cache: "no-store",
    });
    if (!res.ok) {
      console.warn(`[fetchMembers] HTTP ${res.status}`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.warn("[fetchMembers] Failed to reach gateway:", error);
    return null;
  }
}

export async function fetchApiKeys(orgId: string): Promise<ApiKeyEntry[] | null> {
  try {
    const res = await fetch(`${GATEWAY_URL}/v1/orgs/${orgId}/api-keys`, {
      headers: getAuthHeaders(),
      cache: "no-store",
    });
    if (!res.ok) {
      console.warn(`[fetchApiKeys] HTTP ${res.status}`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.warn("[fetchApiKeys] Failed to reach gateway:", error);
    return null;
  }
}

export async function createApiKey(
  orgId: string,
  name: string,
  role?: string,
): Promise<CreateApiKeyResponse | null> {
  try {
    const res = await fetch(`${GATEWAY_URL}/v1/orgs/${orgId}/api-keys`, {
      method: "POST",
      headers: { ...getAuthHeaders(), "Content-Type": "application/json" },
      body: JSON.stringify({ name, role: role ?? "member" }),
    });
    if (!res.ok) {
      console.warn(`[createApiKey] HTTP ${res.status}`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.warn("[createApiKey] Failed to reach gateway:", error);
    return null;
  }
}

export async function revokeApiKey(
  orgId: string,
  keyId: string,
): Promise<boolean> {
  try {
    const res = await fetch(
      `${GATEWAY_URL}/v1/orgs/${orgId}/api-keys/${keyId}`,
      {
        method: "DELETE",
        headers: getAuthHeaders(),
      },
    );
    if (!res.ok) {
      console.warn(`[revokeApiKey] HTTP ${res.status}`);
      return false;
    }
    return true;
  } catch (error) {
    console.warn("[revokeApiKey] Failed to reach gateway:", error);
    return false;
  }
}

export async function fetchAuditLogs(
  orgId: string,
  params?: { limit?: number; offset?: number },
): Promise<AuditLogEntry[] | null> {
  try {
    const query = new URLSearchParams();
    if (params?.limit !== undefined) query.set("limit", String(params.limit));
    if (params?.offset !== undefined) query.set("offset", String(params.offset));
    const qs = query.toString() ? `?${query.toString()}` : "";
    const res = await fetch(`${GATEWAY_URL}/v1/orgs/${orgId}/audit-logs${qs}`, {
      headers: getAuthHeaders(),
      cache: "no-store",
    });
    if (!res.ok) {
      console.warn(`[fetchAuditLogs] HTTP ${res.status}`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.warn("[fetchAuditLogs] Failed to reach gateway:", error);
    return null;
  }
}

export async function fetchOrgStats(
  orgId: string,
  days: number = 30,
): Promise<OrgStats | null> {
  try {
    const res = await fetch(
      `${GATEWAY_URL}/v1/orgs/${orgId}/stats?days=${days}`,
      {
        headers: getAuthHeaders(),
        cache: "no-store",
      },
    );
    if (!res.ok) {
      console.warn(`[fetchOrgStats] HTTP ${res.status}`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.warn("[fetchOrgStats] Failed to reach gateway:", error);
    return null;
  }
}

export async function createTeam(
  orgId: string,
  name: string,
): Promise<TeamEntry | null> {
  try {
    const res = await fetch(`${GATEWAY_URL}/v1/orgs/${orgId}/teams`, {
      method: "POST",
      headers: { ...getAuthHeaders(), "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
    });
    if (!res.ok) {
      console.warn(`[createTeam] HTTP ${res.status}`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.warn("[createTeam] Failed to reach gateway:", error);
    return null;
  }
}

export async function setTeamBudget(
  orgId: string,
  teamId: string,
  budget: { daily_usdc?: number; monthly_usdc?: number },
): Promise<TeamBudget | null> {
  try {
    const res = await fetch(
      `${GATEWAY_URL}/v1/orgs/${orgId}/teams/${teamId}/budget`,
      {
        method: "PUT",
        headers: { ...getAuthHeaders(), "Content-Type": "application/json" },
        body: JSON.stringify(budget),
      },
    );
    if (!res.ok) {
      console.warn(`[setTeamBudget] HTTP ${res.status}`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.warn("[setTeamBudget] Failed to reach gateway:", error);
    return null;
  }
}

export async function fetchTeamBudget(
  orgId: string,
  teamId: string,
): Promise<TeamBudget | null> {
  try {
    const res = await fetch(
      `${GATEWAY_URL}/v1/orgs/${orgId}/teams/${teamId}/budget`,
      {
        headers: getAuthHeaders(),
        cache: "no-store",
      },
    );
    if (!res.ok) {
      console.warn(`[fetchTeamBudget] HTTP ${res.status}`);
      return null;
    }
    return res.json();
  } catch (error) {
    console.warn("[fetchTeamBudget] Failed to reach gateway:", error);
    return null;
  }
}
