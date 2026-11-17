/**
 * Display names for people in the product UI. The account is still the WorkOS
 * user id everywhere else; this only picks what the screen shows.
 *
 * The same workspace and user id always map to the same name, because the pick
 * is a hash. Append to the word lists; reordering or removing a word changes
 * names that were already showing.
 */

const ADJECTIVES = [
  "amber", "azure", "brisk", "calm", "crisp", "dusky", "early", "faint",
  "gentle", "golden", "hazel", "hushed", "ivory", "jade", "keen", "lunar",
  "mellow", "misty", "nimble", "olive", "pale", "plain", "quiet", "rapid",
  "russet", "sandy", "silent", "silver", "slate", "steady", "still", "sturdy",
  "subtle", "swift", "tawny", "teal", "tidy", "umber", "vivid", "warm",
  "wide", "windy", "woven", "bright", "cobalt", "coral", "dawn", "distant",
  "fresh", "grand", "green", "high", "level", "light", "narrow", "north",
  "open", "round", "solid", "south", "spare", "thin", "true", "west",
] as const;

const NOUNS = [
  "basalt", "birch", "cedar", "cinder", "cliff", "comet", "cove", "crane",
  "delta", "dune", "elm", "ember", "fern", "finch", "fjord", "flint",
  "fox", "glade", "granite", "harbor", "heron", "hollow", "juniper", "kestrel",
  "lagoon", "larch", "lichen", "linden", "loon", "maple", "marsh", "meadow",
  "newt", "oak", "orchid", "otter", "pebble", "pine", "plover", "quartz",
  "raven", "reef", "ridge", "river", "sparrow", "spruce", "stone", "swallow",
  "tern", "thistle", "tundra", "willow", "wren", "yarrow", "aspen", "bluff",
  "canyon", "dell", "falcon", "gorge", "isle", "lark", "mesa", "pond",
] as const;

/** Cadri's accents. Variables so the chip follows light and dark. */
const PALETTE = ["var(--pink)", "var(--cyan)", "var(--lav)", "var(--sun)"] as const;

const SYNTHETIC_IDS = new Set(["", "local", "unknown"]);

export type Pseudonym = {
  /** Stable slug, e.g. "amber-heron". */
  slug: string;
  /** Display form, e.g. "Amber Heron". */
  display: string;
  /** Two-letter monogram, e.g. "AH". */
  initials: string;
  /** Accent used to color the name. */
  color: string;
};

function fnv1a(input: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < input.length; index += 1) {
    hash ^= input.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

function titleWord(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1);
}

/**
 * @param workspaceId scope names per team
 * @param userId the person's stable id
 * @param taken slugs already assigned in this workspace
 */
export function pseudonymFor(
  workspaceId: string,
  userId: string,
  taken: ReadonlySet<string> = new Set(),
): Pseudonym {
  let slug = "";
  for (let salt = 0; salt < 1000; salt += 1) {
    const hash = fnv1a(`${workspaceId}:${userId}:${salt}`);
    const adjective = ADJECTIVES[hash % ADJECTIVES.length];
    const noun = NOUNS[Math.floor(hash / ADJECTIVES.length) % NOUNS.length];
    const candidate = `${adjective}-${noun}`;
    if (!taken.has(candidate)) {
      slug = candidate;
      break;
    }
  }
  if (!slug) {
    slug = `user-${fnv1a(`${workspaceId}:${userId}`).toString(36).slice(0, 4)}`;
  }

  const [adjective, noun] = slug.split("-");
  return {
    slug,
    display: `${titleWord(adjective)} ${titleWord(noun ?? "")}`.trim(),
    initials: `${adjective[0]}${(noun ?? "")[0] ?? ""}`.toUpperCase(),
    color: PALETTE[fnv1a(userId) % PALETTE.length],
  };
}

export function workspaceId(orgId: string | null | undefined): string {
  return orgId && orgId.length > 0 ? orgId : "local";
}

export function funName(userId: string, orgId?: string | null): string {
  return pseudonymFor(workspaceId(orgId), userId).slug;
}

/** What to show for a human author. Emails stored on the commit stay off the screen. */
export function displayAuthorName(userId: string, label: string, orgId?: string | null): string {
  if (SYNTHETIC_IDS.has(userId)) {
    return label || "You";
  }
  return funName(userId, orgId);
}

/** Session filter. Marks the signed-in person so the menu stays usable. */
export function sessionUserLabel(userId: string, meId: string | null | undefined, orgId?: string | null): string {
  const name = funName(userId, orgId);
  return meId && userId === meId ? `${name} (you)` : name;
}
