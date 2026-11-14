import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import {
  authHeaders,
  clearToken,
  fetchAuthMode,
  readToken,
  saveToken,
  type AuthMode,
} from "./auth";
import type { InviteResult, Me, OrgResult } from "./ee/workos";
import { CopyButton, INSTALL_DOCS_URL, INSTALL_PROMPT, JOIN_DOCS_URL, JOIN_PROMPT } from "./install";
import { countMySessions, hasTopicFile, onboardingDecision, sessionsReady } from "./onboarding";
import { clearMemoryCaches, primeMemoryBook, setMemoryIdentity } from "./pages/memories/books";

type WorkosModule = typeof import("./ee/workos");

const INSTALL_SKIP_KEY = "cadri.onboarding.install.skipped";

type Gate =
  | { kind: "loading" }
  | { kind: "token"; message: string }
  | { kind: "redirecting" }
  | { kind: "organization" }
  | { kind: "install"; mode: AuthMode; me: Me | null; flow: "install" | "join" }
  | { kind: "error"; message: string }
  | { kind: "app"; mode: AuthMode; me: Me | null };

function installSkipped(): boolean {
  try {
    return localStorage.getItem(INSTALL_SKIP_KEY) === "1";
  } catch {
    return false;
  }
}

function markInstallSkipped(): void {
  try {
    localStorage.setItem(INSTALL_SKIP_KEY, "1");
  } catch {
    /* ignore */
  }
}

function warmMemoryBooks(headers: Record<string, string>, me: Me | null): void {
  setMemoryIdentity(me);
  primeMemoryBook("team", headers);
}

export type Session = {
  mode: AuthMode;
  workos: boolean;
  me: Me | null;
  headers: () => Record<string, string>;
  unauthorized: () => void;
  forgetToken: () => void;
  signOut: () => Promise<string | null>;
  sendInvite: (email: string) => Promise<InviteResult>;
};

const SessionContext = createContext<Session | null>(null);

export function useSession(): Session {
  const value = useContext(SessionContext);
  if (!value) {
    throw new Error("useSession must be used within SessionProvider");
  }
  return value;
}

export function SessionProvider({ children }: { children: ReactNode }) {
  const [gate, setGate] = useState<Gate>({ kind: "loading" });
  const modeRef = useRef<AuthMode>("none");
  const workosRef = useRef<WorkosModule | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const mode = await fetchAuthMode();
      if (cancelled) {
        return;
      }
      modeRef.current = mode;
      if (mode === "none") {
        setGate({ kind: "app", mode, me: null });
        return;
      }
      if (mode === "token") {
        setGate(
          readToken() === null
            ? { kind: "token", message: "Enter the API token for this Cadri server." }
            : { kind: "app", mode, me: null },
        );
        return;
      }
      const api = await import("./ee/workos");
      if (cancelled) {
        return;
      }
      workosRef.current = api;
      const result = await api.loadMe();
      if (cancelled) {
        return;
      }
      if (result.kind === "unauthorized") {
        setGate({ kind: "redirecting" });
        api.signIn();
        return;
      }
      if (result.kind === "no_organization") {
        setGate({ kind: "organization" });
        return;
      }
      if (result.kind === "error") {
        setGate({ kind: "error", message: result.message });
        return;
      }
      const me = result.kind === "ok" ? result.me : null;
      warmMemoryBooks(authHeaders("workos"), me);
      if (me && !installSkipped()) {
        const flow = await detectOnboarding(authHeaders("workos"), me.user_id);
        if (cancelled) {
          return;
        }
        if (flow === "ready") {
          markInstallSkipped();
          setGate({ kind: "app", mode: "workos", me });
          return;
        }
        setGate({ kind: "install", mode: "workos", me, flow });
        return;
      }
      if (!installSkipped()) {
        setGate({ kind: "install", mode: "workos", me, flow: "install" });
        return;
      }
      setGate({ kind: "app", mode: "workos", me });
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const headers = useCallback(() => authHeaders(modeRef.current), []);

  const unauthorized = useCallback(() => {
    if (modeRef.current === "token") {
      clearToken();
      setGate({ kind: "token", message: "The API token was rejected. Enter a valid token." });
      return;
    }
    if (modeRef.current === "workos") {
      setGate({ kind: "redirecting" });
      workosRef.current?.signIn();
    }
  }, []);

  const forgetToken = useCallback(() => {
    if (modeRef.current !== "token") {
      return;
    }
    clearToken();
    setGate({ kind: "token", message: "Token forgotten. Enter an API token to continue." });
  }, []);

  const signOut = useCallback(async (): Promise<string | null> => {
    const api = workosRef.current;
    if (!api) {
      return "Could not sign out.";
    }
    try {
      const target = await api.signOut();
      clearMemoryCaches();
      location.assign(target);
      return null;
    } catch (error) {
      return error instanceof Error ? `Could not sign out: ${error.message}` : "Could not sign out.";
    }
  }, []);

  const sendInvite = useCallback((email: string): Promise<InviteResult> => {
    const api = workosRef.current;
    if (!api) {
      return Promise.resolve({ kind: "error", message: "Could not send the invite. Try again." });
    }
    return api.sendInvite(email);
  }, []);

  const session = useMemo<Session | null>(() => {
    if (gate.kind !== "app") {
      return null;
    }
    return {
      mode: gate.mode,
      workos: gate.mode === "workos",
      me: gate.me,
      headers,
      unauthorized,
      forgetToken,
      signOut,
      sendInvite,
    };
  }, [gate, headers, unauthorized, forgetToken, signOut, sendInvite]);

  if (gate.kind === "loading") {
    return <Boot>Loading…</Boot>;
  }
  if (gate.kind === "redirecting") {
    return <Boot>Redirecting to sign in…</Boot>;
  }
  if (gate.kind === "error") {
    return <Boot>{gate.message}</Boot>;
  }
  if (gate.kind === "token") {
    return (
      <TokenGate
        message={gate.message}
        onSubmit={(value) => {
          saveToken(value);
          modeRef.current = "token";
          setGate({ kind: "app", mode: "token", me: null });
        }}
      />
    );
  }
  if (gate.kind === "organization") {
    return (
      <OrganizationGate
        onSubmit={async (name) => {
          const api = workosRef.current;
          if (!api) {
            return { kind: "error", message: "Could not create the team. Try again." };
          }
          const result = await api.createOrganization(name);
          if (result.kind === "unauthorized") {
            setGate({ kind: "redirecting" });
            api.signIn();
          } else if (result.kind === "ok") {
            warmMemoryBooks(authHeaders("workos"), result.me);
            setGate({ kind: "install", mode: "workos", me: result.me, flow: "install" });
          }
          return result;
        }}
      />
    );
  }
  if (gate.kind === "install") {
    return (
      <InstallGate
        flow={gate.flow}
        orgName={gate.me?.org_name ?? null}
        userId={gate.me?.user_id ?? null}
        onReady={() => {
          markInstallSkipped();
          window.location.assign("/sessions");
        }}
        onSkip={() => {
          markInstallSkipped();
          window.location.assign("/sessions");
        }}
        headers={() => authHeaders(gate.mode)}
      />
    );
  }
  if (!session) {
    return <Boot>Loading…</Boot>;
  }
  return <SessionContext.Provider value={session}>{children}</SessionContext.Provider>;
}

function Boot({ children }: { children: string }) {
  useEffect(() => {
    document.title = "Cadri";
  }, []);
  return (
    <main className="gate">
      <p className="boot">{children}</p>
    </main>
  );
}

function TokenGate({ message, onSubmit }: { message: string; onSubmit: (value: string) => void }) {
  useEffect(() => {
    document.title = "Cadri";
  }, []);

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const value = new FormData(event.currentTarget).get("token");
    if (typeof value !== "string" || value.trim() === "") {
      return;
    }
    onSubmit(value.trim());
  }

  return (
    <main className="gate">
      <form className="panel" onSubmit={submit}>
        <h1>API token</h1>
        <p className="hint">{message}</p>
        <p className="hint">
          The API token is saved in <code>docker/.env</code> and printed when Docker starts Cadri.
          Share it with other people on your local network so they can open Cadri. Anyone with the
          token can read and write sessions.
        </p>
        <label className="field" htmlFor="token-input">
          API token
          <input id="token-input" name="token" type="password" autoComplete="off" required autoFocus />
        </label>
        <button className="btn" type="submit">
          Save
        </button>
      </form>
    </main>
  );
}

function InstallGate({
  flow,
  orgName,
  userId,
  onReady,
  onSkip,
  headers,
}: {
  flow: "install" | "join";
  orgName: string | null;
  userId: string | null;
  onReady: () => void;
  onSkip: () => void;
  headers: () => Record<string, string>;
}) {
  const [status, setStatus] = useState("Waiting for your agent to connect…");
  const joining = flow === "join";
  const prompt = joining ? JOIN_PROMPT : INSTALL_PROMPT;
  const docsUrl = joining ? JOIN_DOCS_URL : INSTALL_DOCS_URL;
  const team = orgName ?? "your team";

  useEffect(() => {
    document.title = joining ? "Cadri · Join" : "Cadri · Install";
  }, [joining]);

  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const response = await fetch("/api/sessions", { headers: headers() });
        if (cancelled) {
          return;
        }
        if (!response.ok) {
          return;
        }
        const rows = (await response.json()) as { user_id?: string }[];
        if (Array.isArray(rows) && sessionsReady(rows, userId)) {
          setStatus("Sessions found — opening Cadri…");
          onReady();
        }
      } catch {
        /* keep polling */
      }
    };
    void tick();
    const id = window.setInterval(() => void tick(), 2000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [headers, onReady, userId]);

  return (
    <main className="gate">
      <div className="panel install-panel">
        <h1>{joining ? `Join ${team}` : "Connect your coding agents"}</h1>
        <p className="hint">
          {joining
            ? `${team} already has a memory book. Paste this prompt into Claude Code, Cursor, Codex, or OpenCode. Your agent installs Cadri, signs this machine in, uploads your recent sessions, and revises the shared book from what those sessions know. Your personal book starts empty. This usually takes several minutes, and longer when the book is large. This page opens Cadri as soon as your sessions arrive, while that revision continues. Edit the prompt to use a smaller window.`
            : "Paste this prompt into Claude Code, Cursor, Codex, or OpenCode. Your agent installs Cadri, signs this machine in, and uploads recent sessions. Seeding memory from those sessions usually takes several minutes. This page opens Cadri as soon as your sessions arrive, while seeding continues. Edit the prompt to use a smaller window."}
        </p>
        <pre className="install-prompt">{prompt}</pre>
        <div className="form-actions">
          <CopyButton text={prompt} />
          <a className="btn ghost" href={docsUrl} target="_blank" rel="noreferrer">
            Open docs
          </a>
          <button className="btn ghost" type="button" onClick={onSkip}>
            Skip for now
          </button>
        </div>
        <p className="form-status" role="status">
          {status}
        </p>
      </div>
    </main>
  );
}

async function detectOnboarding(
  headers: Record<string, string>,
  userId: string,
): Promise<"ready" | "install" | "join"> {
  try {
    const [treeResponse, sessionResponse] = await Promise.all([
      fetch("/api/memories/tree", { headers }),
      fetch("/api/sessions", { headers }),
    ]);
    let teamHasLessons = false;
    let mySessionCount = 0;
    if (treeResponse.ok) {
      const tree = (await treeResponse.json()) as { files?: { path: string }[] };
      teamHasLessons = hasTopicFile(tree.files ?? []);
    }
    if (sessionResponse.ok) {
      const rows = (await sessionResponse.json()) as { user_id?: string }[];
      if (Array.isArray(rows)) {
        mySessionCount = countMySessions(rows, userId);
      }
    }
    return onboardingDecision({ teamHasLessons, mySessionCount });
  } catch {
    return "install";
  }
}

function OrganizationGate({ onSubmit }: { onSubmit: (name: string) => Promise<OrgResult> }) {
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    document.title = "Cadri";
  }, []);

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const value = new FormData(event.currentTarget).get("name");
    const name = typeof value === "string" ? value.trim() : "";
    if (!name) {
      return;
    }
    setNote("Creating the team…");
    setBusy(true);
    void onSubmit(name).then((result) => {
      if (result.kind === "error") {
        setNote(result.message);
        setBusy(false);
      }
    });
  }

  return (
    <main className="gate">
      <form className="panel" onSubmit={submit}>
        <h1>Name your team to open Cadri.</h1>
        <p className="hint">
          Name the team this account belongs to. Sessions are shared with that team. Someone
          joins this team only when they accept an email invite. Signing up without an invite starts
          a separate team. The team includes a 30-day trial. After that, Cadri is billed monthly
          for each active member.
        </p>
        <label className="field" htmlFor="org-name">
          Team name
          <input
            id="org-name"
            name="name"
            required
            maxLength={100}
            autoComplete="organization"
            autoFocus
          />
        </label>
        <div className="form-actions">
          <button className="btn" type="submit" disabled={busy}>
            Create team
          </button>
          <p className="form-status" role="status">
            {note}
          </p>
        </div>
      </form>
    </main>
  );
}
