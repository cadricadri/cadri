import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { INSTALL_PROMPT, JOIN_PROMPT } from "../../install";
import { EmptyBook } from "./EmptyBook";

afterEach(() => {
  cleanup();
});

function prompt(): string {
  return document.querySelector("pre.install-prompt")?.textContent ?? "";
}

describe("empty memory book", () => {
  it("asks a new team to install and shows the install prompt", () => {
    render(<EmptyBook scope="team" phase="install" />);
    expect(screen.getByRole("heading", { name: "Connect your coding agents" })).toBeTruthy();
    expect(prompt()).toBe(INSTALL_PROMPT);
    expect(screen.getByRole("link", { name: "Open install docs" }).getAttribute("href")).toContain(
      "/docs/installation",
    );
  });

  it("asks someone joining a book to revise it, not to seed a new one", () => {
    render(<EmptyBook scope="team" phase="install" flow="join" />);
    expect(screen.getByRole("heading", { name: "Join your team's memory" })).toBeTruthy();
    expect(prompt()).toBe(JOIN_PROMPT);
    expect(screen.queryByText(/cadri-memory-seed/)).toBeNull();
    expect(screen.getByRole("link", { name: "How joining works" }).getAttribute("href")).toContain(
      "/docs/joining",
    );
  });

  it("tells a seeded personal book to stay on the join skill", () => {
    render(<EmptyBook scope="personal" phase="empty" flow="join" />);
    expect(screen.getByRole("heading", { name: "No personal memories yet" })).toBeTruthy();
    expect(screen.getByText(/cadri-memory-join/)).toBeTruthy();
  });
});
