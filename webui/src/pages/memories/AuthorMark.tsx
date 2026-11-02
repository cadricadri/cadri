import { HumanMark, RobotMark } from "../../icons";
import type { MemoryAuthor } from "./types";

export function shortRobotId(id: string): string {
  return id.slice(0, 8);
}

export function AuthorMark({ author }: { author: MemoryAuthor }) {
  if (author.kind === "robot") {
    return (
      <span className="author-mark" title={author.id}>
        <RobotMark />
        <span>{shortRobotId(author.id)}</span>
      </span>
    );
  }
  return (
    <span className="author-mark" title={author.label}>
      <HumanMark />
      <span>{author.label}</span>
    </span>
  );
}
