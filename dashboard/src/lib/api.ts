import { getApiKey } from "@/lib/auth";
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
    const apiKey = getApiKey();
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
  team_id?: string;
  hourly_limit?: number | null;
  daily_limit?: number | null;
  monthly_limit?: number | null;
  hourly_spend?: number;
  daily_spend?: number;
  monthly_spend?: number;
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

// ─── Structured error result types ───────────────────────────────────────────

export interface ApiError {
  status: number;
  type: string;
  message: string;
}

export type ApiResult<T> = { ok: true; data: T } | { ok: false; error: ApiError };

export async function fetchApi<T>(url: string, init?: RequestInit): Promise<ApiResult<T>> {
  try {
    const res = await fetch(url, init);
    if (!res.ok) {
      let type = 'unknown';
      let message = `HTTP ${res.status}`;
      try {
        const body = await res.json() as { error?: { type?: string; message?: string } | string };
        if (body?.error && typeof body.error === 'object' && body.error.type) {
          type = body.error.type;
          message = body.error.message ?? message;
        } else if (typeof body?.error === 'string') {
          message = body.error;
        }
      } catch {
        // Response body wasn't JSON — use status text
      }
      return { ok: false, error: { status: res.status, type, message } };
    }
    return { ok: true, data: await res.json() as T };
  } catch (err) {
    return {
      ok: false,
      error: {
        status: 0,
        type: 'network_error',
        message: err instanceof Error ? err.message : 'Network error',
      },
    };
  }
}

// ─── Org / team / audit API functions ────────────────────────────────────────

export async function fetchOrgs(): Promise<ApiResult<OrgEntry[]>> {
  return fetchApi<OrgEntry[]>(`${GATEWAY_URL}/v1/orgs`, {
    headers: getAuthHeaders(),
    cache: 'no-store',
  });
}

export async function fetchTeams(orgId: string): Promise<ApiResult<TeamEntry[]>> {
  return fetchApi<TeamEntry[]>(`${GATEWAY_URL}/v1/orgs/${orgId}/teams`, {
    headers: getAuthHeaders(),
    cache: 'no-store',
  });
}

export async function fetchMembers(orgId: string): Promise<ApiResult<MemberEntry[]>> {
  return fetchApi<MemberEntry[]>(`${GATEWAY_URL}/v1/orgs/${orgId}/members`, {
    headers: getAuthHeaders(),
    cache: 'no-store',
  });
}

export async function fetchApiKeys(orgId: string): Promise<ApiResult<ApiKeyEntry[]>> {
  return fetchApi<ApiKeyEntry[]>(`${GATEWAY_URL}/v1/orgs/${orgId}/api-keys`, {
    headers: getAuthHeaders(),
    cache: 'no-store',
  });
}

export async function createApiKey(
  orgId: string,
  name: string,
  role?: string,
): Promise<ApiResult<CreateApiKeyResponse>> {
  return fetchApi<CreateApiKeyResponse>(`${GATEWAY_URL}/v1/orgs/${orgId}/api-keys`, {
    method: 'POST',
    headers: { ...getAuthHeaders(), 'Content-Type': 'application/json' },
    body: JSON.stringify({ name, role: role ?? 'member' }),
  });
}

export async function revokeApiKey(
  orgId: string,
  keyId: string,
): Promise<ApiResult<boolean>> {
  const result = await fetchApi<unknown>(
    `${GATEWAY_URL}/v1/orgs/${orgId}/api-keys/${keyId}`,
    { method: 'DELETE', headers: getAuthHeaders() },
  );
  if (result.ok) return { ok: true, data: true };
  return result;
}

export async function fetchAuditLogs(
  orgId: string,
  params?: { limit?: number },
): Promise<ApiResult<AuditLogEntry[]>> {
  const query = new URLSearchParams();
  if (params?.limit !== undefined) query.set('limit', String(params.limit));
  const qs = query.toString() ? `?${query.toString()}` : '';
  return fetchApi<AuditLogEntry[]>(`${GATEWAY_URL}/v1/orgs/${orgId}/audit-logs${qs}`, {
    headers: getAuthHeaders(),
    cache: 'no-store',
  });
}

export async function fetchOrgStats(
  orgId: string,
  days: number = 30,
): Promise<ApiResult<OrgStats>> {
  return fetchApi<OrgStats>(`${GATEWAY_URL}/v1/orgs/${orgId}/stats?days=${days}`, {
    headers: getAuthHeaders(),
    cache: 'no-store',
  });
}

export async function createTeam(
  orgId: string,
  name: string,
): Promise<ApiResult<TeamEntry>> {
  return fetchApi<TeamEntry>(`${GATEWAY_URL}/v1/orgs/${orgId}/teams`, {
    method: 'POST',
    headers: { ...getAuthHeaders(), 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  });
}

export async function setTeamBudget(
  orgId: string,
  teamId: string,
  budget: { hourly?: number; daily?: number; monthly?: number },
): Promise<ApiResult<TeamBudget>> {
  return fetchApi<TeamBudget>(`${GATEWAY_URL}/v1/orgs/${orgId}/teams/${teamId}/budget`, {
    method: 'PUT',
    headers: { ...getAuthHeaders(), 'Content-Type': 'application/json' },
    body: JSON.stringify(budget),
  });
}

export async function fetchTeamBudget(
  orgId: string,
  teamId: string,
): Promise<ApiResult<TeamBudget>> {
  return fetchApi<TeamBudget>(`${GATEWAY_URL}/v1/orgs/${orgId}/teams/${teamId}/budget`, {
    headers: getAuthHeaders(),
    cache: 'no-store',
  });
}
