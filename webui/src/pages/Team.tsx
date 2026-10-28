import { useState, type FormEvent } from "react";
import { Page } from "../page";
import { useSession } from "../session";

export default function Team() {
  const session = useSession();
  const [email, setEmail] = useState("");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const value = email.trim();
    if (!value) {
      return;
    }
    setNote("Sending invite…");
    setBusy(true);
    void session.sendInvite(value).then((result) => {
      if (result.kind === "unauthorized") {
        session.unauthorized();
        return;
      }
      setBusy(false);
      if (result.kind === "error") {
        setNote(result.message);
        return;
      }
      setEmail("");
      setNote(`Invite sent to ${result.email}.`);
    });
  }

  return (
    <Page title="Team">
      <form className="stack" onSubmit={submit}>
        <label className="field" htmlFor="invite-email">
          Teammate's email
          <input
            id="invite-email"
            type="email"
            autoComplete="email"
            required
            placeholder="teammate@company.com"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
          />
        </label>
        <div className="form-actions">
          <button className="btn" type="submit" disabled={busy}>
            Send invite
          </button>
          <p className="form-status" role="status">
            {note}
          </p>
        </div>
      </form>
    </Page>
  );
}
