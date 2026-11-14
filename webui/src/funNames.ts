/**
 * Display names for people in the product UI. The account is still the WorkOS
 * user id everywhere else; this only picks what the screen shows. The same id
 * always maps to the same name, on every browser, because the pick is a hash.
 */
const FUN_NAMES = [
  "DividedByZero",
  "ParityErrorCheck",
  "DeviceNotReady",
  "BlueScreenOfDeath",
  "GeneralProtectionFaulter",
  "IllegalInstructor",
  "SyntaxErrorInLiner",
  "FileNotFoundFinder",
  "WriteProtectBreaker",
  "FourZeroFourNotFound",
  "FiveZeroZeroInternal",
  "GatewayTimeouter",
  "ConnectionResetter",
  "BrokenPiper",
  "CorsPolicyBlocker",
  "DnsProbeFaulter",
  "TooManyRequester",
  "MixedContentFetcher",
  "NullPointerExceptioner",
  "SegFaulter",
  "BufferOverflower",
  "MemoryLeaker",
  "OutOfMemoryKiller",
  "DanglingPointer",
  "UseAfterFreer",
  "RaceConditioner",
  "DeadlockDetector",
  "CompilerPanicker",
  "UndefinedBehavior",
  "MergeConflicter",
  "UnhandledRejecter",
  "InfiniteLooper",
  "OffByOner",
  "NullCoalescer",
  "TypeMismatcher",
  "IndentationError",
  "MaximumStackExceeder",
  "DeprecatedWarner",
  "FeatureCreeper",
  "TrailingWhitespacer",
  "HardcodedSecreter",
  "DetachedHeader",
] as const;

/** Ids that are not a person: keep the label the page already has. */
const SYNTHETIC_IDS = new Set(["", "local", "unknown"]);

export function funName(userId: string): string {
  let hash = 2166136261;
  for (let index = 0; index < userId.length; index += 1) {
    hash ^= userId.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return FUN_NAMES[(hash >>> 0) % FUN_NAMES.length];
}

/** What to show for a human author. Emails stored on the commit stay off the screen. */
export function displayAuthorName(userId: string, label: string): string {
  if (SYNTHETIC_IDS.has(userId)) {
    return label || "You";
  }
  return funName(userId);
}

/** Session lists and the session page. Marks the signed-in person so the filter stays usable. */
export function sessionUserLabel(userId: string, meId: string | null | undefined): string {
  const name = funName(userId);
  return meId && userId === meId ? `${name} (you)` : name;
}
