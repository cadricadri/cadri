import { type CSSProperties } from "react";
import { pseudonymFor, workspaceId } from "../../funNames";
import { useSession } from "../../session";

/** A person's display name with their Cadri accent. */
export function UserName({ userId }: { userId: string }) {
  const session = useSession();
  const person = pseudonymFor(workspaceId(session.me?.org_id), userId);
  const you = session.me?.user_id === userId;
  return (
    <span className="user-name" style={{ "--user-color": person.color } as CSSProperties}>
      <i className="user-swatch" aria-hidden="true" />
      <span>{person.slug}</span>
      {you ? <span className="user-you"> (you)</span> : null}
    </span>
  );
}
