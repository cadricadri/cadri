import { Link } from "react-router";
import { HumanMark, RobotMark } from "../../icons";
import { sessionLink } from "../sessions/types";
import type { MemoryAuthor } from "./types";

export function shortRobotId(id: string): string {
  return id.slice(0, 8);
}

/** The author's mark. With `sessionId`, it links to the session that wrote the commit. */
export function AuthorMark({ author, sessionId }: { author: MemoryAuthor; sessionId?: string }) {
  const robot = author.kind === "robot";
  const name = robot ? shortRobotId(author.id) : author.label;
  const title = robot ? author.id : author.label;
  const body = (
    <>
      {robot ? <RobotMark /> : <HumanMark />}
      <span>{name}</span>
    </>
  );
  if (sessionId) {
    return (
      <Link className="author-mark" to={sessionLink(sessionId)} title={`${title} · open the session that wrote this`}>
        {body}
      </Link>
    );
  }
  return (
    <span className="author-mark" title={title}>
      {body}
    </span>
  );
}
