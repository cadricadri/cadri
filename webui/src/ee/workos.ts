import type { Auth, AuthContext } from "../auth";

type Me = {
  user_id: string;
  email: string | null;
  org_id: string;
  org_name: string | null;
};

function signIn(): void {
  const returnTo = `${location.pathname}${location.search}`;
  location.assign(`/auth/login?return_to=${encodeURIComponent(returnTo)}`);
}

async function signOut(ctx: AuthContext): Promise<void> {
  try {
    const response = await fetch("/auth/logout", { method: "POST" });
    if (!response.ok) {
      throw new Error(`${response.status} ${response.statusText}`);
    }
    const { logout_url } = (await response.json()) as { logout_url: string };
    location.assign(logout_url);
  } catch (error) {
    ctx.show(error instanceof Error ? `Could not sign out: ${error.message}` : "Could not sign out.");
  }
}

function renderAccount(ctx: AuthContext, me: Me): void {
  const account = document.createElement("span");
  account.className = "account";
  account.textContent = [me.email ?? me.user_id, me.org_name ?? me.org_id].join(" · ");
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = "Sign out";
  button.addEventListener("click", () => {
    void signOut(ctx);
  });
  ctx.actions.prepend(account, button);
}

export async function workosAuth(ctx: AuthContext): Promise<Auth> {
  let redirecting = false;
  const auth: Auth = {
    ready: () => !redirecting,
    headers: () => ({}),
    unauthorized: () => {
      redirecting = true;
      ctx.show("Redirecting to sign in…");
      signIn();
    },
  };

  try {
    const response = await fetch("/api/me");
    if (response.status === 401) {
      auth.unauthorized();
    } else if (response.ok) {
      renderAccount(ctx, (await response.json()) as Me);
    }
  } catch {
    // The transcript request reports connectivity problems.
  }
  return auth;
}
