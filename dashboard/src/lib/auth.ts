const API_KEY_STORAGE_KEY = "solvela_api_key";
const LEGACY_KEY = "rcr_api_key";

// Read the API key, migrating from the legacy `rcr_api_key` slot if present.
// Migration window: keep this fallback until 2026-08-01, then drop it.
export function getApiKey(): string | null {
  if (typeof window === "undefined") return null;
  const current = localStorage.getItem(API_KEY_STORAGE_KEY);
  if (current) return current;
  const legacy = localStorage.getItem(LEGACY_KEY);
  if (legacy) {
    localStorage.setItem(API_KEY_STORAGE_KEY, legacy);
    localStorage.removeItem(LEGACY_KEY);
    return legacy;
  }
  return null;
}

export function setApiKey(key: string): void {
  localStorage.setItem(API_KEY_STORAGE_KEY, key);
  localStorage.removeItem(LEGACY_KEY);
}

export function clearApiKey(): void {
  localStorage.removeItem(API_KEY_STORAGE_KEY);
  localStorage.removeItem(LEGACY_KEY);
}

export function hasApiKey(): boolean {
  return !!getApiKey();
}
