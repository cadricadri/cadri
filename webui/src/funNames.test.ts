import { describe, expect, it } from "vitest";
import { displayAuthorName, pseudonymFor, sessionUserLabel } from "./funNames";

describe("fun names", () => {
  it("keeps one slug for a person and picks another when that slug is taken", () => {
    const first = pseudonymFor("org-1", "user-1");
    expect(pseudonymFor("org-1", "user-1")).toEqual(first);
    expect(first.slug).toMatch(/^[a-z]+-[a-z]+$/);
    expect(first.display).toBe(
      first.slug
        .split("-")
        .map((word) => word[0].toUpperCase() + word.slice(1))
        .join(" "),
    );
    const taken = pseudonymFor("org-1", "user-1", new Set([first.slug]));
    expect(taken.slug).not.toBe(first.slug);
  });

  it("hides raw ids and marks the signed-in person", () => {
    expect(displayAuthorName("local", "Ada", "org-1")).toBe("Ada");
    expect(displayAuthorName("", "", "org-1")).toBe("You");
    const name = pseudonymFor("org-1", "user-1").slug;
    expect(sessionUserLabel("user-1", "user-1", "org-1")).toBe(`${name} (you)`);
    expect(sessionUserLabel("user-1", "someone-else", "org-1")).toBe(name);
  });
});
