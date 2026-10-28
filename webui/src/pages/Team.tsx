import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent, type RefObject } from "react";
import { Page } from "../page";
import { useSession } from "../session";

type Status = "joined" | "invited" | "expired";

type Person = {
  email: string;
  status: Status;
};

const STATUS_LABEL: Record<Status, string> = {
  joined: "Joined",
  invited: "Invited",
  expired: "Invite Expired",
};

const RANK: Record<Status, number> = {
  joined: 0,
  invited: 1,
  expired: 2,
};

const SAMPLE: Person[] = [
  { email: "sam@company.com", status: "joined" },
  { email: "new@company.com", status: "invited" },
  { email: "old@company.com", status: "expired" },
];

function EmailText({ email }: { email: string }) {
  const at = email.indexOf("@");
  if (at < 1) {
    return email;
  }
  return (
    <>
      {email.slice(0, at + 1)}
      <wbr />
      {email.slice(at + 1)}
    </>
  );
}

function sameEmail(left: string, right: string): boolean {
  return left.toLowerCase() === right.toLowerCase();
}

function sortPeople(people: Person[], you: string | null): Person[] {
  return [...people].sort((left, right) => {
    const leftYou = you !== null && sameEmail(left.email, you);
    const rightYou = you !== null && sameEmail(right.email, you);
    if (leftYou !== rightYou) {
      return leftYou ? -1 : 1;
    }
    if (left.status !== right.status) {
      return RANK[left.status] - RANK[right.status];
    }
    return left.email.localeCompare(right.email);
  });
}

function initialPeople(you: string | null): Person[] {
  const people = SAMPLE.filter((person) => you === null || !sameEmail(person.email, you));
  if (you) {
    people.unshift({ email: you, status: "joined" });
  }
  return sortPeople(people, you);
}

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => {
    window.setTimeout(resolve, ms);
  });
}

export default function Team() {
  const session = useSession();
  const you = session.me?.email?.trim() || null;
  const [people, setPeople] = useState<Person[]>(() => initialPeople(you));
  const [email, setEmail] = useState("");
  const [note, setNote] = useState("");
  const [sent, setSent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [fresh, setFresh] = useState<{ email: string; at: number } | null>(null);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    if (!fresh) {
      return;
    }
    const timer = window.setTimeout(() => setFresh(null), 2400);
    return () => window.clearTimeout(timer);
  }, [fresh]);

  function markSent(address: string, again: boolean): void {
    setNote(`${again ? "Invite sent again" : "Invite sent"} to ${address}.`);
    setSent(true);
    setFresh({ email: address, at: Date.now() });
    setPending(null);
  }

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    const value = email.trim();
    if (!value || busy) {
      return;
    }
    const existing = people.find((person) => sameEmail(person.email, value));
    if (existing?.status === "joined") {
      setSent(false);
      setNote("That person is already on this team.");
      return;
    }
    if (existing?.status === "invited") {
      setSent(false);
      setNote("An invite is already waiting for that email.");
      return;
    }
    setBusy(true);
    setSent(false);
    setNote("Sending invite…");
    await wait(450);
    if (!mounted.current) {
      return;
    }
    setBusy(false);
    setEmail("");
    if (existing) {
      setPeople((current) =>
        sortPeople(
          current.map((person) =>
            sameEmail(person.email, existing.email) ? { ...person, status: "invited" } : person,
          ),
          you,
        ),
      );
      markSent(existing.email, true);
      return;
    }
    setPeople((current) => sortPeople([...current, { email: value, status: "invited" }], you));
    markSent(value, false);
  }

  function resend(person: Person): void {
    if (busy) {
      return;
    }
    setPeople((current) =>
      sortPeople(
        current.map((item) =>
          sameEmail(item.email, person.email) ? { ...item, status: "invited" } : item,
        ),
        you,
      ),
    );
    markSent(person.email, true);
  }

  function remove(person: Person): void {
    setPeople((current) => current.filter((item) => !sameEmail(item.email, person.email)));
    setPending(null);
    setSent(false);
    setNote(
      person.status === "joined"
        ? `Removed ${person.email} from this team.`
        : `Revoked the invite to ${person.email}.`,
    );
  }

  return (
    <Page title="Team">
      <form className="team-invite" onSubmit={(event) => void submit(event)}>
        <label className="team-invite-label" htmlFor="invite-email">
          Add your teammate!
        </label>
        <input
          id="invite-email"
          type="email"
          autoComplete="email"
          required
          placeholder="teammate@company.com"
          value={email}
          onChange={(event) => setEmail(event.target.value)}
        />
        <button className="btn" type="submit" disabled={busy}>
          {busy ? "Sending…" : "Send Invite"}
        </button>
        <div className="team-invite-note">
          <p className={sent ? "team-sent" : "form-status"} role="status">
            {note}
          </p>
        </div>
      </form>
      <h2 className="team-heading">People</h2>
      <div className="team-table-wrap">
        <table className="team-table">
          <thead>
            <tr>
              <th scope="col">email address</th>
              <th scope="col">status</th>
              <th scope="col">admin action</th>
            </tr>
          </thead>
          <tbody>
            {people.map((person) => (
              <PersonRow
                key={person.email}
                person={person}
                you={you !== null && sameEmail(person.email, you)}
                fresh={fresh !== null && sameEmail(fresh.email, person.email)}
                confirming={pending !== null && sameEmail(pending, person.email)}
                busy={busy}
                onAsk={() => setPending(person.email)}
                onCancel={() => setPending(null)}
                onResend={() => resend(person)}
                onRemove={() => remove(person)}
              />
            ))}
          </tbody>
        </table>
      </div>
    </Page>
  );
}

function PersonRow({
  person,
  you,
  fresh,
  confirming,
  busy,
  onAsk,
  onCancel,
  onResend,
  onRemove,
}: {
  person: Person;
  you: boolean;
  fresh: boolean;
  confirming: boolean;
  busy: boolean;
  onAsk: () => void;
  onCancel: () => void;
  onResend: () => void;
  onRemove: () => void;
}) {
  const confirmRef = useRef<HTMLButtonElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const restoreFocus = useRef(false);
  const joined = person.status === "joined";

  useEffect(() => {
    if (confirming) {
      confirmRef.current?.focus();
    }
  }, [confirming]);

  useEffect(() => {
    if (!confirming && restoreFocus.current) {
      restoreFocus.current = false;
      triggerRef.current?.focus();
    }
  }, [confirming]);

  function cancel(): void {
    restoreFocus.current = true;
    onCancel();
  }

  function onKey(event: KeyboardEvent<HTMLDivElement>): void {
    if (event.key === "Escape") {
      event.stopPropagation();
      cancel();
    }
  }

  return (
    <>
      <tr className={fresh ? "team-row-new" : undefined}>
        <td>
          <span className="team-email">
            <EmailText email={person.email} />
          </span>
          {you ? <span className="team-you">You</span> : null}
        </td>
        <td>
          <span className={`team-status team-status-${person.status}`}>{STATUS_LABEL[person.status]}</span>
        </td>
        <td>
          {you || confirming ? null : (
            <ManageMenu
              email={person.email}
              joined={joined}
              expired={person.status === "expired"}
              busy={busy}
              buttonRef={triggerRef}
              onAsk={onAsk}
              onResend={onResend}
            />
          )}
        </td>
      </tr>
      {confirming ? (
        <tr className="team-confirm-row">
          <td colSpan={3}>
            <div className="team-confirm" onKeyDown={onKey}>
              <p>{joined ? `Remove ${person.email} from this team?` : `Revoke the invite to ${person.email}?`}</p>
              <button ref={confirmRef} className="btn team-confirm-btn" type="button" onClick={onRemove}>
                {joined ? "Remove" : "Revoke"}
              </button>
              <button className="btn ghost team-confirm-btn" type="button" onClick={cancel}>
                Cancel
              </button>
            </div>
          </td>
        </tr>
      ) : null}
    </>
  );
}

function ManageMenu({
  email,
  joined,
  expired,
  busy,
  buttonRef,
  onAsk,
  onResend,
}: {
  email: string;
  joined: boolean;
  expired: boolean;
  busy: boolean;
  buttonRef: RefObject<HTMLButtonElement | null>;
  onAsk: () => void;
  onResend: () => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) {
      return;
    }
    menuRef.current?.querySelector<HTMLButtonElement>("button")?.focus();
    function onPointer(event: MouseEvent): void {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    }
    function onKey(event: globalThis.KeyboardEvent): void {
      if (event.key === "Escape") {
        setOpen(false);
        buttonRef.current?.focus();
      }
    }
    document.addEventListener("mousedown", onPointer);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onPointer);
      document.removeEventListener("keydown", onKey);
    };
  }, [buttonRef, open]);

  return (
    <div className="team-manage" ref={rootRef}>
      <button
        ref={buttonRef}
        className="team-manage-btn"
        type="button"
        disabled={busy}
        aria-expanded={open}
        aria-haspopup="menu"
        aria-label={`Manage ${email}`}
        onClick={() => setOpen((value) => !value)}
      >
        …
      </button>
      {open ? (
        <div className="team-menu" role="menu" ref={menuRef}>
          {expired ? (
            <button
              role="menuitem"
              type="button"
              onClick={() => {
                setOpen(false);
                onResend();
              }}
            >
              Resend
            </button>
          ) : null}
          <button
            role="menuitem"
            type="button"
            onClick={() => {
              setOpen(false);
              onAsk();
            }}
          >
            {joined ? "Remove" : "Revoke"}
          </button>
        </div>
      ) : null}
    </div>
  );
}
