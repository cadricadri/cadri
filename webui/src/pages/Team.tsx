import { useCallback, useEffect, useRef, useState, type FormEvent, type KeyboardEvent, type RefObject } from "react";
import {
  loadTeam,
  removeMember,
  resendInvitation,
  revokeInvitation,
  sendInvite,
  type TeamActionResult,
  type TeamPerson,
  type TeamStatus,
} from "../ee/workos";
import { Page } from "../page";
import { useSession } from "../session";

type Person = TeamPerson;

const STATUS_LABEL: Record<TeamStatus, string> = {
  joined: "Joined",
  invited: "Invited",
  expired: "Invite Expired",
  revoked: "Revoked",
  inactive: "Inactive",
};

const RANK: Record<TeamStatus, number> = {
  joined: 0,
  invited: 1,
  expired: 2,
  revoked: 3,
  inactive: 4,
};

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

function rowKey(person: Person): string {
  return `${person.status}:${person.membershipId ?? ""}:${person.invitationId ?? ""}:${person.email}`;
}

function isMember(status: TeamStatus): boolean {
  return status === "joined" || status === "inactive";
}

export default function Team() {
  const session = useSession();
  const you = session.me?.email?.trim() || null;
  const unauthorized = session.unauthorized;
  const [people, setPeople] = useState<Person[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [email, setEmail] = useState("");
  const [note, setNote] = useState("");
  const [sent, setSent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [fresh, setFresh] = useState<{ email: string; at: number } | null>(null);
  const mounted = useRef(true);

  const reload = useCallback(async (): Promise<boolean> => {
    const result = await loadTeam();
    if (!mounted.current) {
      return false;
    }
    if (result.kind === "unauthorized") {
      unauthorized();
      return false;
    }
    if (result.kind === "error") {
      setLoadError(result.message);
      setPeople([]);
      setLoaded(true);
      return false;
    }
    setLoadError("");
    setPeople(sortPeople(result.people, you));
    setLoaded(true);
    return true;
  }, [unauthorized, you]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

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

  function applyAction(result: TeamActionResult): boolean {
    if (!mounted.current) {
      return false;
    }
    setBusy(false);
    if (result.kind === "unauthorized") {
      unauthorized();
      return false;
    }
    if (result.kind === "error") {
      setSent(false);
      setNote(result.message);
      return false;
    }
    return true;
  }

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    const value = email.trim();
    if (!value || busy) {
      return;
    }
    const existing = people.find((person) => sameEmail(person.email, value));
    if (existing && isMember(existing.status)) {
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
    const result = await sendInvite(value);
    if (!mounted.current) {
      return;
    }
    if (result.kind === "unauthorized") {
      setBusy(false);
      unauthorized();
      return;
    }
    if (result.kind === "error") {
      setBusy(false);
      setNote(result.message);
      return;
    }
    setBusy(false);
    setEmail("");
    const refreshed = await reload();
    if (!mounted.current) {
      return;
    }
    if (!refreshed) {
      setSent(true);
      setNote(`Invite sent to ${result.email}, but the team list could not be refreshed.`);
      return;
    }
    markSent(result.email, existing !== undefined);
  }

  async function resend(person: Person): Promise<void> {
    if (busy || !person.invitationId) {
      return;
    }
    setBusy(true);
    setSent(false);
    setNote("Sending invite…");
    const result = await resendInvitation(person.invitationId);
    if (!applyAction(result)) {
      return;
    }
    const refreshed = await reload();
    if (!mounted.current) {
      return;
    }
    if (!refreshed) {
      setSent(true);
      setNote(`Invite sent again to ${person.email}, but the team list could not be refreshed.`);
      return;
    }
    markSent(person.email, true);
  }

  async function remove(person: Person): Promise<void> {
    if (busy) {
      return;
    }
    const member = isMember(person.status);
    setBusy(true);
    setSent(false);
    setNote(member ? "Removing…" : "Revoking…");
    const result = member
      ? person.membershipId
        ? await removeMember(person.membershipId)
        : { kind: "error" as const, message: "Could not update the team. Try again." }
      : person.invitationId
        ? await revokeInvitation(person.invitationId)
        : { kind: "error" as const, message: "Could not update the team. Try again." };
    if (!applyAction(result)) {
      return;
    }
    setPending(null);
    const refreshed = await reload();
    if (!mounted.current) {
      return;
    }
    setSent(false);
    if (member) {
      setNote(
        refreshed
          ? `Removed ${person.email} from this team.`
          : `Removed ${person.email}, but the team list could not be refreshed.`,
      );
      return;
    }
    setNote(
      refreshed
        ? `Revoked the invite to ${person.email}.`
        : `Revoked the invite to ${person.email}, but the team list could not be refreshed.`,
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
          placeholder="name@example.com"
          value={email}
          onChange={(event) => setEmail(event.target.value)}
        />
        <button className="btn" type="submit" disabled={busy}>
          {busy && note === "Sending invite…" ? "Sending…" : "Send Invite"}
        </button>
        <div className="team-invite-note">
          <p className={sent ? "team-sent" : "form-status"} role="status">
            {note}
          </p>
        </div>
      </form>
      <h2 className="team-heading">People</h2>
      {loadError ? (
        <p className="form-status" role="status">
          {loadError}
        </p>
      ) : null}
      {!loaded && !loadError ? (
        <p className="team-empty" role="status">
          Loading…
        </p>
      ) : null}
      {loaded && !loadError && people.length === 0 ? (
        <p className="team-empty" role="status">
          No one is on this team yet.
        </p>
      ) : null}
      {people.length > 0 ? (
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
                  key={rowKey(person)}
                  person={person}
                  you={you !== null && sameEmail(person.email, you)}
                  fresh={fresh !== null && sameEmail(fresh.email, person.email)}
                  confirming={pending !== null && pending === rowKey(person)}
                  busy={busy}
                  onAsk={() => setPending(rowKey(person))}
                  onCancel={() => setPending(null)}
                  onResend={() => void resend(person)}
                  onRemove={() => void remove(person)}
                />
              ))}
            </tbody>
          </table>
        </div>
      ) : null}
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
  const member = isMember(person.status);
  const canResend =
    (person.status === "expired" || person.status === "revoked") && person.invitationId !== null;
  const canRevoke =
    (person.status === "invited" || person.status === "expired") && person.invitationId !== null;
  const canRemove = member && person.membershipId !== null;
  const manageable = canResend || canRevoke || canRemove;

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
          {person.name ? <span className="team-name">{person.name}</span> : null}
          {you ? <span className="team-you">You</span> : null}
        </td>
        <td>
          <span className={`team-status team-status-${person.status}`}>{STATUS_LABEL[person.status]}</span>
        </td>
        <td>
          {you || confirming || !manageable ? null : (
            <ManageMenu
              email={person.email}
              canResend={canResend}
              canRevoke={canRevoke}
              canRemove={canRemove}
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
              <p>{member ? `Remove ${person.email} from this team?` : `Revoke the invite to ${person.email}?`}</p>
              <button ref={confirmRef} className="btn team-confirm-btn" type="button" disabled={busy} onClick={onRemove}>
                {member ? "Remove" : "Revoke"}
              </button>
              <button className="btn ghost team-confirm-btn" type="button" disabled={busy} onClick={cancel}>
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
  canResend,
  canRevoke,
  canRemove,
  busy,
  buttonRef,
  onAsk,
  onResend,
}: {
  email: string;
  canResend: boolean;
  canRevoke: boolean;
  canRemove: boolean;
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
          {canResend ? (
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
          {canRevoke || canRemove ? (
            <button
              role="menuitem"
              type="button"
              onClick={() => {
                setOpen(false);
                onAsk();
              }}
            >
              {canRemove ? "Remove" : "Revoke"}
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
