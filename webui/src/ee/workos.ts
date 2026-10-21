import type { Auth, AuthContext } from "../auth";

type Me = {
  user_id: string;
  email: string | null;
  org_id: string;
  org_name: string | null;
};

const ORG_ERRORS: Record<string, string> = {
  invalid_organization_name: "Enter a team name.",
  multiple_organizations: "This account is already on more than one team. Switching teams isn't available yet.",
  workos_unavailable: "Could not create the team. Try again.",
  workos_rejected: "Could not create the team. Try again.",
};

const INVITE_ERRORS: Record<string, string> = {
  invalid_email: "Enter an email address.",
  already_member: "That person is already on this team.",
  already_invited: "An invite is already waiting for that email.",
  no_organization: "Create a team before inviting someone.",
  workos_unavailable: "Could not send the invite. Try again.",
  workos_rejected: "Could not send the invite. Try again.",
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

async function errorCode(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: unknown };
    return typeof body.error === "string" ? body.error : "";
  } catch {
    return "";
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
  const invite = document.querySelector<HTMLFormElement>("#invite-form");
  if (invite) {
    invite.hidden = false;
  }
}

function wireInvite(ctx: AuthContext): void {
  const form = document.querySelector<HTMLFormElement>("#invite-form");
  const input = document.querySelector<HTMLInputElement>("#invite-email");
  const note = document.querySelector<HTMLSpanElement>("#invite-status");
  if (!form || !input || form.dataset.wired === "true") {
    return;
  }
  form.dataset.wired = "true";
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const email = input.value.trim();
    if (!email) {
      return;
    }
    if (note) {
      note.textContent = "Sending invite…";
    }
    const submit = form.querySelector<HTMLButtonElement>("button[type=submit]");
    if (submit) {
      submit.disabled = true;
    }
    void (async () => {
      try {
        const response = await fetch("/auth/invitations", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ email }),
        });
        if (response.status === 401) {
          signIn();
          return;
        }
        if (!response.ok) {
          const code = await errorCode(response);
          if (note) {
            note.textContent = INVITE_ERRORS[code] ?? "Could not send the invite. Try again.";
          }
          return;
        }
        const sent = (await response.json()) as { email?: string };
        input.value = "";
        if (note) {
          note.textContent = `Invite sent to ${sent.email ?? email}.`;
        }
      } catch {
        if (note) {
          note.textContent = "Could not send the invite. Try again.";
        }
      } finally {
        if (submit) {
          submit.disabled = false;
        }
      }
    })();
  });
}

function showOrgForm(ctx: AuthContext, onReady: (me: Me) => void): void {
  const form = document.querySelector<HTMLFormElement>("#org-form");
  const input = document.querySelector<HTMLInputElement>("#org-name");
  const note = document.querySelector<HTMLParagraphElement>("#org-status");
  const table = document.querySelector<HTMLTableElement>("#transcripts");
  if (!form || !input) {
    ctx.show("This account isn't on a team yet.");
    return;
  }
  form.hidden = false;
  if (table) {
    table.hidden = true;
  }
  const refresh = document.querySelector<HTMLButtonElement>("#refresh");
  if (refresh) {
    refresh.hidden = true;
  }
  ctx.show("Name your team to open Cadri.");
  if (form.dataset.wired === "true") {
    return;
  }
  form.dataset.wired = "true";
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const name = input.value.trim();
    if (!name) {
      return;
    }
    if (note) {
      note.textContent = "Creating the team…";
    }
    const submit = form.querySelector<HTMLButtonElement>("button[type=submit]");
    if (submit) {
      submit.disabled = true;
    }
    void (async () => {
      try {
        const response = await fetch("/auth/organization", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ name }),
        });
        if (response.status === 401) {
          signIn();
          return;
        }
        const code = response.ok ? "" : await errorCode(response);
        if (response.status === 409 && code === "already_in_organization") {
          const meResponse = await fetch("/api/me");
          if (meResponse.ok) {
            form.hidden = true;
            if (table) {
              table.hidden = false;
            }
            const refresh = document.querySelector<HTMLButtonElement>("#refresh");
            if (refresh) {
              refresh.hidden = false;
            }
            onReady((await meResponse.json()) as Me);
          } else if (note) {
            note.textContent = "This account is already on a team.";
          }
          return;
        }
        if (!response.ok) {
          if (note) {
            note.textContent = ORG_ERRORS[code] ?? "Could not create the team. Try again.";
          }
          return;
        }
        const me = (await response.json()) as Me;
        form.hidden = true;
        if (note) {
          note.textContent = "";
        }
        if (table) {
          table.hidden = false;
        }
        const refresh = document.querySelector<HTMLButtonElement>("#refresh");
        if (refresh) {
          refresh.hidden = false;
        }
        onReady(me);
      } catch {
        if (note) {
          note.textContent = "Could not create the team. Try again.";
        }
      } finally {
        if (submit) {
          submit.disabled = false;
        }
      }
    })();
  });
}

export async function workosAuth(ctx: AuthContext): Promise<Auth> {
  let redirecting = false;
  let needsOrg = false;
  wireInvite(ctx);
  const auth: Auth = {
    ready: () => !redirecting && !needsOrg,
    headers: () => ({}),
    unauthorized: () => {
      redirecting = true;
      ctx.show("Redirecting to sign in…");
      signIn();
    },
  };

  const ready = (me: Me): void => {
    needsOrg = false;
    renderAccount(ctx, me);
    ctx.reload();
  };

  try {
    const response = await fetch("/api/me");
    if (response.status === 401) {
      auth.unauthorized();
    } else if (response.status === 403) {
      const code = await errorCode(response);
      if (code === "no_organization") {
        needsOrg = true;
        showOrgForm(ctx, ready);
      } else {
        ctx.show(code ? `Could not load the account (${code}).` : "Could not load the account.");
      }
    } else if (response.ok) {
      renderAccount(ctx, (await response.json()) as Me);
    }
  } catch {
    // The transcript request reports connectivity problems.
  }
  return auth;
}
