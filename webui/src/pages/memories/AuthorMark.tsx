import { Link } from "react-router";
import { HumanMark, RobotMark } from "../../icons";
import type { MemoryAuthor } from "./types";

export function shortSession(sessionId: string): string {
  return sessionId.slice(0, 8);
}

export function AuthorMark({ author }: { author: MemoryAuthor }) {
  if (author.kind === "robot") {
    return (
      <Link
        className="author-mark"
        to={`/transcripts?session=${encodeURIComponent(author.session_id)}`}
        title={author.session_id}
      >
        <RobotMark />
        <span>{shortSession(author.session_id)}</span>
      </Link>
    );
  }
  return (
    <span className="author-mark" title={author.label}>
      <HumanMark />
      <span>{author.label}</span>
    </span>
  );
}
