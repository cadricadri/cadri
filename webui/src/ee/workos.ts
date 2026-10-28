export type Me = {
  user_id: string;
  email: string | null;
  org_id: string;
  org_name: string | null;
};

export type MeResult =
  | { kind: "ok"; me: Me }
  | { kind: "unauthorized" }
  | { kind: "no_organization" }
  | { kind: "error"; message: string }
  | { kind: "continue" };

export type OrgResult =
  | { kind: "ok"; me: Me }
  | { kind: "unauthorized" }
  | { kind: "error"; message: string };

export type InviteResult =
  | { kind: "ok"; email: string }
  | { kind: "unauthorized" }
  | { kind: "error"; message: string };

const ORG_ERRORS: Record<string, string> = {
  invalid_organization_name: "Enter a team name.",
  multiple_organizations: "This account is already on more than one team. Switching teams isn't available yet.",
  workos_unavailable: "Could not create the team. Try again.",
  workos_rejected: "Could not create the team. Try again.",
  stripe_unavailable: "Could not start the trial. Try again.",
  stripe_rejected: "Could not start the trial. Try again.",
  billing_unavailable: "Could not start the trial. Try again.",
};

const INVITE_ERRORS: Record<string, string> = {
  invalid_email: "Enter an email address.",
  already_member: "That person is already on this team.",
  already_invited: "An invite is already waiting for that email.",
  no_organization: "Create a team before inviting someone.",
  workos_unavailable: "Could not send the invite. Try again.",
  workos_rejected: "Could not send the invite. Try again.",
  payment_required: "This team's trial has ended. Open Billing to add a payment method.",
};

export function signIn(): void {
  const returnTo = `${location.pathname}${location.search}`;
  location.assign(`/auth/login?return_to=${encodeURIComponent(returnTo)}`);
}

export async function signOut(): Promise<string> {
  const response = await fetch("/auth/logout", { method: "POST" });
  if (!response.ok) {
    throw new Error(`${response.status} ${response.statusText}`);
  }
  const { logout_url } = (await response.json()) as { logout_url: string };
  return logout_url;
}

async function errorCode(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: unknown };
    return typeof body.error === "string" ? body.error : "";
  } catch {
    return "";
  }
}

export async function loadMe(): Promise<MeResult> {
  try {
    const response = await fetch("/api/me");
    if (response.status === 401) {
      return { kind: "unauthorized" };
    }
    if (response.status === 403) {
      const code = await errorCode(response);
      if (code === "no_organization") {
        return { kind: "no_organization" };
      }
      return {
        kind: "error",
        message: code ? `Could not load the account (${code}).` : "Could not load the account.",
      };
    }
    if (!response.ok) {
      return { kind: "continue" };
    }
    return { kind: "ok", me: (await response.json()) as Me };
  } catch {
    return { kind: "continue" };
  }
}

export async function createOrganization(name: string): Promise<OrgResult> {
  try {
    const response = await fetch("/auth/organization", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
    });
    if (response.status === 401) {
      return { kind: "unauthorized" };
    }
    const code = response.ok ? "" : await errorCode(response);
    if (response.status === 409 && code === "already_in_organization") {
      const meResponse = await fetch("/api/me");
      if (meResponse.ok) {
        return { kind: "ok", me: (await meResponse.json()) as Me };
      }
      return { kind: "error", message: "This account is already on a team." };
    }
    if (!response.ok) {
      return {
        kind: "error",
        message: ORG_ERRORS[code] ?? "Could not create the team. Try again.",
      };
    }
    return { kind: "ok", me: (await response.json()) as Me };
  } catch {
    return { kind: "error", message: "Could not create the team. Try again." };
  }
}

export async function sendInvite(email: string): Promise<InviteResult> {
  try {
    const response = await fetch("/auth/invitations", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email }),
    });
    if (response.status === 401) {
      return { kind: "unauthorized" };
    }
    if (!response.ok) {
      const code = await errorCode(response);
      return {
        kind: "error",
        message: INVITE_ERRORS[code] ?? "Could not send the invite. Try again.",
      };
    }
    const sent = (await response.json()) as { email?: string };
    return { kind: "ok", email: sent.email ?? email };
  } catch {
    return { kind: "error", message: "Could not send the invite. Try again." };
  }
}
