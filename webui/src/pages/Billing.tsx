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

type BillingInvoice = {
  id: string;
  date: number;
  description: string;
  amount: string;
  currency: string;
  receipt_url: string | null;
};

export default function Billing() {
  const session = useSession();
  const [params] = useSearchParams();
  const [account, setAccount] = useState<BillingAccount | null>(null);
  const [invoices, setInvoices] = useState<BillingInvoice[] | null>(null);
  const [note, setNote] = useState("Loading…");
  const [historyNote, setHistoryNote] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const portal = params.get("portal_return") === "1" ? "?portal_return=1" : "";
    let cancelled = false;
    void (async () => {
      setInvoices(null);
      setHistoryNote("");
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
        return;
      }
      try {
        const response = await fetch("/api/billing/invoices");
        if (cancelled) {
          return;
        }
        if (response.status === 401) {
          session.unauthorized();
          return;
        }
        if (!response.ok) {
          throw new Error(`${response.status}`);
        }
        const body = (await response.json()) as { invoices?: BillingInvoice[] };
        setInvoices(Array.isArray(body.invoices) ? body.invoices : []);
      } catch {
        if (!cancelled) {
          setInvoices([]);
          setHistoryNote("Could not load billing history.");
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
        <>
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
          <section className="billing-history" aria-labelledby="billing-history-heading">
            <h2 id="billing-history-heading">Billing History</h2>
            {historyNote ? (
              <p className="billing-history-empty" role="status">
                {historyNote}
              </p>
            ) : invoices === null ? (
              <p className="billing-history-empty" role="status">
                Loading…
              </p>
            ) : invoices.length === 0 ? (
              <p className="billing-history-empty">No invoices yet.</p>
            ) : (
              <div className="billing-table-wrap">
                <table className="billing-table">
                  <thead>
                    <tr>
                      <th>Date</th>
                      <th>Description</th>
                      <th>Amount</th>
                      <th>Receipt</th>
                    </tr>
                  </thead>
                  <tbody>
                    {invoices.map((invoice) => {
                      const receipt = receiptHref(invoice.receipt_url);
                      return (
                        <tr key={invoice.id}>
                          <td>{formatDate(invoice.date)}</td>
                          <td>{invoice.description}</td>
                          <td className="billing-amount">{invoice.amount}</td>
                          <td>
                            {receipt ? (
                              <a
                                className="text-link"
                                href={receipt}
                                target="_blank"
                                rel="noopener noreferrer"
                              >
                                Receipt
                              </a>
                            ) : (
                              <span aria-label="No receipt">—</span>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )}
          </section>
        </>
      ) : null}
    </Page>
  );
}

function formatDate(unix: number): string {
  const date = new Date(unix * 1000);
  if (Number.isNaN(date.getTime())) {
    return "—";
  }
  return date.toLocaleDateString(undefined, {
    year: "numeric",
    month: "long",
    day: "numeric",
  });
}

function receiptHref(url: string | null): string | null {
  if (!url) {
    return null;
  }
  try {
    const parsed = new URL(url);
    const host = parsed.hostname.toLowerCase();
    if (parsed.protocol !== "https:") {
      return null;
    }
    if (host !== "stripe.com" && !host.endsWith(".stripe.com")) {
      return null;
    }
    return parsed.toString();
  } catch {
    return null;
  }
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
