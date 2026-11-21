import { describe, expect, it } from "vitest";
import { memoryFileLocation, resolveMemoryLink } from "./links";

describe("resolveMemoryLink", () => {
  it("resolves a relative link against the open file's directory", () => {
    expect(resolveMemoryLink("components/MEMORY.md", "page/how-it-works.md")).toEqual({
      path: "components/page/how-it-works.md",
      hash: "",
    });
  });

  it("resolves ./ and ../ within the book", () => {
    expect(resolveMemoryLink("components/auth/MEMORY.md", "./gotchas.md")).toEqual({
      path: "components/auth/gotchas.md",
      hash: "",
    });
    expect(resolveMemoryLink("components/page/MEMORY.md", "../how-it-works.md")).toEqual({
      path: "components/how-it-works.md",
      hash: "",
    });
  });

  it("treats a leading slash as the book root", () => {
    expect(resolveMemoryLink("components/MEMORY.md", "/architecture/conventions.md")).toEqual({
      path: "architecture/conventions.md",
      hash: "",
    });
  });

  it("keeps a heading on the resolved file", () => {
    expect(resolveMemoryLink("components/MEMORY.md", "page/how-it-works.md#setup")).toEqual({
      path: "components/page/how-it-works.md",
      hash: "setup",
    });
  });

  it("decodes encoded path segments", () => {
    expect(resolveMemoryLink("components/MEMORY.md", "page/how%20it%20works.md")).toEqual({
      path: "components/page/how it works.md",
      hash: "",
    });
  });

  it("leaves external, in-page, and escaping links alone", () => {
    expect(resolveMemoryLink("components/MEMORY.md", "https://example.com/docs")).toBeNull();
    expect(resolveMemoryLink("components/MEMORY.md", "mailto:ada@example.com")).toBeNull();
    expect(resolveMemoryLink("components/MEMORY.md", "//example.com/docs")).toBeNull();
    expect(resolveMemoryLink("components/MEMORY.md", "#local")).toBeNull();
    expect(resolveMemoryLink("components/MEMORY.md", "../../outside.md")).toBeNull();
    expect(resolveMemoryLink("components/MEMORY.md", "")).toBeNull();
  });
});

describe("memoryFileLocation", () => {
  it("points the file browser at the resolved path", () => {
    const search = new URLSearchParams("file=components/MEMORY.md");
    expect(memoryFileLocation("components/MEMORY.md", "page/how-it-works.md", search)).toBe(
      "/?file=components%2Fpage%2Fhow-it-works.md",
    );
  });

  it("keeps the book scope, drops the commit log, and keeps a heading", () => {
    const search = new URLSearchParams("scope=personal&view=committed&file=components/MEMORY.md");
    expect(memoryFileLocation("components/MEMORY.md", "page/how-it-works.md#setup", search)).toBe(
      "/?scope=personal&file=components%2Fpage%2Fhow-it-works.md#setup",
    );
  });
});
