export type AuthMode = "none" | "token" | "workos";

export type Auth = {
  ready(): boolean;
  headers(): Record<string, string>;
  unauthorized(): void;
};

export type AuthContext = {
  actions: HTMLElement;
  show(message: string): void;
  reload(): void;
};

const TOKEN_KEY = "cadri.apiToken";

export const noAuth: Auth = {
  ready: () => true,
  headers: () => ({}),
  unauthorized: () => {},
};

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

export function tokenAuth(ctx: AuthContext): Auth {
  const form = document.querySelector<HTMLFormElement>("#token-form");
  const input = document.querySelector<HTMLInputElement>("#token-input");
  const forget = document.createElement("button");
  forget.type = "button";
  forget.textContent = "Forget token";
  ctx.actions.append(forget);

  const token = (): string | null => localStorage.getItem(TOKEN_KEY);
  const sync = (): void => {
    forget.hidden = token() === null;
  };
  const prompt = (message: string): void => {
    localStorage.removeItem(TOKEN_KEY);
    sync();
    ctx.show(message);
    if (form && input) {
      form.hidden = false;
      input.value = "";
      input.focus();
    }
  };

  form?.addEventListener("submit", (event) => {
    event.preventDefault();
    const value = input?.value.trim() ?? "";
    if (!value) {
      return;
    }
    localStorage.setItem(TOKEN_KEY, value);
    form.hidden = true;
    sync();
    ctx.reload();
  });
  forget.addEventListener("click", () => {
    prompt("Token forgotten. Enter an API token to continue.");
  });

  if (token() === null) {
    prompt("Enter the API token for this Cadri server.");
  }
  sync();

  return {
    ready: () => token() !== null,
    headers: (): Record<string, string> => {
      const value = token();
      return value ? { Authorization: `Bearer ${value}` } : {};
    },
    unauthorized: () => prompt("The API token was rejected. Enter a valid token."),
  };
}
