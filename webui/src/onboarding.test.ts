import { describe, expect, it } from "vitest";
import { countMySessions, hasTopicFile, onboardingDecision, sessionsReady } from "./onboarding";

describe("onboarding", () => {
  it("treats an index-only book as not yet seeded", () => {
    expect(hasTopicFile([{ path: "MEMORY.md" }, { path: "components/README.md" }])).toBe(false);
    expect(hasTopicFile([{ path: "components/billing.md" }])).toBe(true);
  });

  it("sends someone with sessions past the gate, and a lesson book to join", () => {
    expect(onboardingDecision({ teamHasLessons: true, mySessionCount: 2 })).toBe("ready");
    expect(onboardingDecision({ teamHasLessons: true, mySessionCount: 0 })).toBe("join");
    expect(onboardingDecision({ teamHasLessons: false, mySessionCount: 0 })).toBe("install");
    expect(countMySessions([{ user_id: "a" }, { user_id: "b" }, { user_id: "a" }], "a")).toBe(2);
  });

  it("counts any session when the page does not know who is signed in", () => {
    expect(sessionsReady([], "user-1")).toBe(false);
    expect(sessionsReady([{ user_id: "other" }], "user-1")).toBe(false);
    expect(sessionsReady([{ user_id: "other" }], null)).toBe(true);
    expect(sessionsReady([{ user_id: "user-1" }], "user-1")).toBe(true);
  });
});
