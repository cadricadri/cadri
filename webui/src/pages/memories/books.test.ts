import { afterEach, describe, expect, it, vi } from "vitest";
import {
  LoadError,
  clearMemoryCaches,
  loadMemoryBook,
  peekMemoryBook,
  rememberMemoryBook,
  setMemoryIdentity,
  storeFromTree,
} from "./books";
import type { MemoryStore, MemoryTree } from "./types";

const tree: MemoryTree = {
  tip: "abc",
  seq: 2,
  files: [{ path: "components/billing.md", blob: "blob-1", size: 4 }],
  directories: ["components"],
  open: { path: "components/billing.md", blob: "blob-1", content: "Retries.\n" },
};

function store(overrides: Partial<MemoryStore> = {}): MemoryStore {
  return {
    tip: "abc",
    files: [
      {
        path: "components/billing.md",
        blob: "blob-1",
        content: "Retries.\n",
        blame: [{ author: { kind: "human", user_id: "u", label: "Ada" }, commit_id: "c1" }],
      },
    ],
    directories: ["components"],
    commits: [],
    ...overrides,
  };
}

afterEach(() => {
  clearMemoryCaches();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("memory books", () => {
  it("keeps blame for an unchanged file and drops it when the blob moves", () => {
    const kept = storeFromTree(tree, store());
    expect(kept.files[0]?.blame).toHaveLength(1);
    expect(kept.files[0]?.content).toBe("Retries.\n");
    const moved = storeFromTree(
      { ...tree, files: [{ path: "components/billing.md", blob: "blob-2", size: 5 }] },
      store(),
    );
    expect(moved.files[0]?.blame).toEqual([]);
    expect(moved.files[0]?.content).toBeNull();
  });

  it("reuses a fresh book and accepts a matching etag without rewriting it", async () => {
    let now = 1_000_000;
    vi.spyOn(Date, "now").mockImplementation(() => now);
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => Response.json(tree));
    vi.stubGlobal("fetch", fetchMock);

    const first = await loadMemoryBook("team", { authorization: "Bearer t" }, "components/billing.md");
    const second = await loadMemoryBook("team", {}, null);
    expect(second).toBe(first);
    expect(fetchMock).toHaveBeenCalledTimes(1);

    now += 11_000;
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 304 }));
    const revalidated = await loadMemoryBook("team", {}, null);
    expect(revalidated.tip).toBe("abc");
    expect(fetchMock).toHaveBeenCalledTimes(2);
    const sent = fetchMock.mock.calls[1]?.[1];
    expect(new Headers(sent?.headers).get("if-none-match")).toBe('"abc"');
  });

  it("turns a locked or signed-out book into an error the page can show", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("no", { status: 402 })));
    await expect(loadMemoryBook("personal", {}, null)).rejects.toBeInstanceOf(LoadError);
    vi.stubGlobal("fetch", vi.fn(async () => new Response("no", { status: 401 })));
    await expect(loadMemoryBook("team", {}, null)).rejects.toMatchObject({ status: 401 });
  });

  it("stores each org separately and forgets them on sign-out", () => {
    const me = { user_id: "user", email: "a@cadri.io", org_id: "org", org_name: "Org" };
    setMemoryIdentity(me);
    rememberMemoryBook("team", store());
    expect(localStorage.getItem("cadri.memories.v1.org.user.team")).toContain("billing.md");
    expect(peekMemoryBook("personal")).toBeNull();
    clearMemoryCaches();
    expect(localStorage.getItem("cadri.memories.v1.org.user.team")).toBeNull();
    expect(peekMemoryBook("team")).toBeNull();
  });
});
