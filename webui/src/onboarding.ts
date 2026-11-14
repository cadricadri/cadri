/** A lesson is any markdown file that is not a directory index. Same rule as the CLI. */
export function hasTopicFile(files: { path: string }[]): boolean {
  return files.some((file) => {
    const name = file.path.slice(file.path.lastIndexOf("/") + 1);
    return name !== "MEMORY.md" && name !== "README.md";
  });
}

export function countMySessions(rows: { user_id?: string }[], userId: string): number {
  return rows.reduce((count, row) => count + (row.user_id === userId ? 1 : 0), 0);
}

/**
 * A person who already uploaded sessions is done with this gate.
 * A team book that already has lessons is revised, not seeded.
 * Anyone else is seeding a new book.
 */
export function onboardingDecision(input: {
  teamHasLessons: boolean;
  mySessionCount: number;
}): "ready" | "install" | "join" {
  if (input.mySessionCount > 0) {
    return "ready";
  }
  if (input.teamHasLessons) {
    return "join";
  }
  return "install";
}

/** The install screen opens once this person's sessions exist. With no user id, any session counts. */
export function sessionsReady(rows: { user_id?: string }[], userId: string | null): boolean {
  if (rows.length === 0) {
    return false;
  }
  if (!userId) {
    return true;
  }
  return rows.some((row) => row.user_id === userId);
}
