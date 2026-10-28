export type AuthMode = "none" | "token" | "workos";

const TOKEN_KEY = "cadri.apiToken";

export async function fetchAuthMode(): Promise<AuthMode> {
  try {
    const response = await fetch("/api/auth/config");
    if (!response.ok) {
      return "none";
    }
    const { mode } = (await response.json()) as { mode?: unknown };
    return mode === "token" || mode === "workos" ? mode : "none";
  } catch {
    return "none";
  }
}

export function readToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
}

export function saveToken(value: string): void {
  localStorage.setItem(TOKEN_KEY, value);
}

export function clearToken(): void {
  localStorage.removeItem(TOKEN_KEY);
}

export function authHeaders(mode: AuthMode): Record<string, string> {
  if (mode !== "token") {
    return {};
  }
  const value = readToken();
  return value ? { Authorization: `Bearer ${value}` } : {};
}
