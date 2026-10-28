import { useEffect, useState } from "react";
import { useSearchParams } from "react-router";
import { Page } from "../page";
import { useSession } from "../session";

type BillingAccount = {
  status: string;
  trial_end: number | null;
  seat_count: number | null;
  access: boolean;
};

export default function Billing() {
  const session = useSession();
  const [params] = useSearchParams();
  const [account, setAccount] = useState<BillingAccount | null>(null);
  const [note, setNote] = useState("Loading…");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const portal = params.get("portal_return") === "1" ? "?portal_return=1" : "";
    let cancelled = false;
    void (async () => {
      try {
        const response = await fetch(`/api/billing${portal}`);
        if (cancelled) {
          return;
        }
        if (response.status === 401) {
          session.unauthorized();
          return;
        }
        if (response.status === 404) {
          setAccount(null);
          setNote("Billing is not turned on for this server.");
          return;
        }
        if (!response.ok) {
          throw new Error(`${response.status}`);
        }
        setAccount((await response.json()) as BillingAccount);
        setNote("");
      } catch {
        if (!cancelled) {
          setAccount(null);
          setNote("Could not load billing.");
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [params, session]);

  async function openPortal(): Promise<void> {
    setBusy(true);
    setNote("");
    try {
      const response = await fetch("/api/billing/portal", { method: "POST" });
      if (response.status === 401) {
        session.unauthorized();
        return;
      }
      if (!response.ok) {
        throw new Error(`${response.status}`);
      }
      const body = (await response.json()) as { url?: string };
      if (!body.url) {
        throw new Error("missing url");
      }
      location.assign(body.url);
    } catch {
      setNote("Could not open the billing portal.");
      setBusy(false);
    }
  }

  const action = account?.status === "active" ? "Manage billing" : "Add payment method";

  return (
    <Page title="Billing">
      <p className="lede">{account ? describe(account) : note}</p>
      {account ? (
        <div className="form-actions">
          <button className="btn" type="button" disabled={busy} onClick={() => void openPortal()}>
            {action}
          </button>
          {note ? (
            <p className="form-status" role="status">
              {note}
            </p>
          ) : null}
        </div>
      ) : null}
    </Page>
  );
}

function describe(account: BillingAccount): string {
  const seats =
    account.seat_count === null
      ? "each active member"
      : account.seat_count === 1
        ? "1 active member"
        : `${account.seat_count} active members`;
  const ends =
    account.trial_end === null
      ? null
      : new Date(account.trial_end * 1000).toLocaleDateString(undefined, {
          year: "numeric",
          month: "long",
          day: "numeric",
        });
  if (account.status === "trialing" && ends) {
    return `Trial ends ${ends}. Cadri is free until then, then billed monthly for ${seats}.`;
  }
  if (account.status === "active") {
    return `This team is paying monthly for ${seats}.`;
  }
  return "This team's trial has ended. Add a payment method to continue. Billing is monthly for each active member.";
}
