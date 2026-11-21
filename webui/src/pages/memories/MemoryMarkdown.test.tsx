import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { MemoryRouter } from "react-router";
import { MemoryMarkdown } from "./MemoryMarkdown";

afterEach(() => {
  cleanup();
});

function renderMarkdown(entry: string, markdown: string, path = "components/MEMORY.md"): void {
  render(
    <MemoryRouter initialEntries={[entry]}>
      <MemoryMarkdown path={path}>{markdown}</MemoryMarkdown>
    </MemoryRouter>,
  );
}

describe("memory markdown links", () => {
  it("opens a relative file link in the file browser", () => {
    renderMarkdown("/?file=components%2FMEMORY.md", "See [how it works](page/how-it-works.md).");
    expect(screen.getByRole("link", { name: "how it works" }).getAttribute("href")).toBe(
      "/?file=components%2Fpage%2Fhow-it-works.md",
    );
  });

  it("leaves web links as written", () => {
    renderMarkdown("/?file=components%2FMEMORY.md", "See [the docs](https://example.com/docs).");
    expect(screen.getByRole("link", { name: "the docs" }).getAttribute("href")).toBe("https://example.com/docs");
  });
});
