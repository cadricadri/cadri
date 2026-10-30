import { lazy, Suspense, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Link, NavLink, Navigate, Route, Routes, useLocation } from "react-router";
import { NavGlyph } from "./icons";
import { Page } from "./page";
import { useSession } from "./session";

const Memories = lazy(() => import("./pages/Memories"));
const Transcripts = lazy(() => import("./pages/Transcripts"));
const Usage = lazy(() => import("./pages/Usage"));
const Team = lazy(() => import("./pages/Team"));
const Billing = lazy(() => import("./pages/Billing"));

const NARROW = "(max-width: 959px)";

function useNarrowScreen(): boolean {
  const [matches, setMatches] = useState(() => window.matchMedia(NARROW).matches);
  useEffect(() => {
    const media = window.matchMedia(NARROW);
    const onChange = () => setMatches(media.matches);
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, []);
  return matches;
}

function linkClass({ isActive }: { isActive: boolean }): string {
  return isActive ? "nav-link active" : "nav-link";
}

export function App() {
  return (
    <Shell>
      <Suspense fallback={<p className="status">Loading…</p>}>
        <Routes>
          <Route path="/" element={<Memories />} />
          <Route path="/transcripts" element={<Transcripts />} />
          <Route path="/usage" element={<Usage />} />
          <Route
            path="/team"
            element={
              <WorkosRoute>
                <Team />
              </WorkosRoute>
            }
          />
          <Route
            path="/billing"
            element={
              <WorkosRoute>
                <Billing />
              </WorkosRoute>
            }
          />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </Suspense>
    </Shell>
  );
}

function WorkosRoute({ children }: { children: ReactNode }) {
  const session = useSession();
  if (!session.workos) {
    return <Navigate to="/" replace />;
  }
  return children;
}

function Shell({ children }: { children: ReactNode }) {
  const session = useSession();
  const narrow = useNarrowScreen();
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLButtonElement>(null);
  const navRef = useRef<HTMLElement>(null);
  const wasOpen = useRef(false);
  const { pathname } = useLocation();
  const access = useBillingAccess(session.workos, pathname);

  useEffect(() => {
    setOpen(false);
  }, [pathname]);

  useEffect(() => {
    if (!narrow) {
      setOpen(false);
    }
  }, [narrow]);

  useEffect(() => {
    if (!open || !narrow) {
      return;
    }
    const frame = requestAnimationFrame(() => {
      navRef.current?.querySelector<HTMLElement>("a.nav-link")?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [open, narrow]);

  useEffect(() => {
    if (wasOpen.current && !open && narrow) {
      menuRef.current?.focus();
    }
    wasOpen.current = open;
  }, [open, narrow]);

  useEffect(() => {
    if (!open || !narrow) {
      return;
    }
    function onKey(event: globalThis.KeyboardEvent): void {
      if (event.key === "Escape") {
        setOpen(false);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, narrow]);

  function onTab(event: KeyboardEvent<HTMLElement>): void {
    if (!narrow || !open || event.key !== "Tab" || !navRef.current) {
      return;
    }
    const focusable = Array.from(navRef.current.querySelectorAll<HTMLElement>("a, button")).filter(
      (element) => !element.hasAttribute("disabled"),
    );
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) {
      return;
    }
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return (
    <div className="app">
      <a className="skip" href="#content">
        Skip to content
      </a>
      {narrow && open ? (
        <button className="scrim" type="button" aria-label="Close menu" onClick={() => setOpen(false)} />
      ) : null}
      <aside
        id="app-nav"
        className={open ? "nav open" : "nav"}
        ref={navRef}
        inert={narrow && !open}
        onKeyDown={onTab}
      >
        <NavLink to="/" end className="brand">
          <img className="logo" src="/favicon/favicon-32.png" alt="" width={32} height={32} />
          Cadri
        </NavLink>
        <nav className="nav-links" aria-label="Pages">
          <NavLink to="/" end className={linkClass} data-tone="pink">
            <NavGlyph name="memories" />
            Memories
          </NavLink>
          <NavLink to="/transcripts" className={linkClass} data-tone="cyan">
            <NavGlyph name="transcripts" />
            Transcripts
          </NavLink>
          <NavLink to="/usage" className={linkClass} data-tone="sun">
            <NavGlyph name="usage" />
            Usage Statistics
          </NavLink>
          <div className="nav-rule" role="separator" />
          {session.workos ? (
            <NavLink to="/team" className={linkClass} data-tone="lav">
              <NavGlyph name="team" />
              Team
            </NavLink>
          ) : null}
          <a className="nav-link" data-tone="cyan" href="https://cadri.io/docs">
            <NavGlyph name="docs" />
            Docs
          </a>
          {session.workos ? (
            <NavLink to="/billing" className={linkClass} data-tone="pink">
              <NavGlyph name="billing" />
              Billing
            </NavLink>
          ) : null}
        </nav>
        <AccountFooter />
      </aside>
      <div className="main">
        <header className="topbar">
          <button
            ref={menuRef}
            className="menu"
            type="button"
            aria-expanded={open}
            aria-controls="app-nav"
            aria-label={open ? "Close menu" : "Open menu"}
            onClick={() => setOpen((value) => !value)}
          >
            <span />
            <span />
            <span />
          </button>
          <NavLink to="/" end className="topbar-title">
            <img className="logo" src="/favicon/favicon-32.png" alt="" width={24} height={24} />
            Cadri
          </NavLink>
        </header>
        <div id="content" className="content" tabIndex={-1} inert={narrow && open}>
          {access === "locked" && pathname !== "/billing" ? <TrialEnded /> : children}
        </div>
      </div>
    </div>
  );
}

type BillingAccess = "unknown" | "open" | "locked";

function useBillingAccess(workos: boolean, pathname: string): BillingAccess {
  const [access, setAccess] = useState<BillingAccess>(workos ? "unknown" : "open");

  useEffect(() => {
    if (!workos) {
      setAccess("open");
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const response = await fetch("/api/billing");
        if (cancelled) {
          return;
        }
        if (!response.ok) {
          setAccess("open");
          return;
        }
        const body = (await response.json()) as { access?: boolean };
        setAccess(body.access === false ? "locked" : "open");
      } catch {
        if (!cancelled) {
          setAccess("open");
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [workos, pathname]);

  return access;
}

function TrialEnded() {
  return (
    <Page title="Trial ended">
      <p className="lede">This team's trial has ended. Add a payment method to open Cadri again.</p>
      <p className="lede">
        <Link className="text-link" to="/billing">
          Go to Billing
        </Link>
      </p>
    </Page>
  );
}

function AccountFooter() {
  const session = useSession();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (session.mode === "token") {
    return (
      <footer className="nav-footer">
        <button className="btn ghost" type="button" onClick={session.forgetToken}>
          Forget token
        </button>
        <SupportLine />
      </footer>
    );
  }
  if (session.mode !== "workos" || !session.me) {
    return (
      <footer className="nav-footer">
        <SupportLine />
      </footer>
    );
  }
  const me = session.me;
  return (
    <footer className="nav-footer">
      <p className="account">{[me.email ?? me.user_id, me.org_name ?? me.org_id].join(" · ")}</p>
      {error ? (
        <p className="form-status" role="status">
          {error}
        </p>
      ) : null}
      <button
        className="btn ghost"
        type="button"
        disabled={busy}
        onClick={() => {
          setBusy(true);
          setError(null);
          void session.signOut().then((message) => {
            if (message) {
              setError(message);
              setBusy(false);
            }
          });
        }}
      >
        Sign out
      </button>
      <SupportLine />
    </footer>
  );
}

const themeKey = "cadri-theme";

type ThemeChoice = "auto" | "dark" | "light";

function readTheme(): ThemeChoice {
  try {
    const value = localStorage.getItem(themeKey);
    return value === "dark" || value === "light" ? value : "auto";
  } catch {
    return "auto";
  }
}

function applyTheme(choice: ThemeChoice): void {
  if (choice === "auto") {
    document.documentElement.removeAttribute("data-theme");
    return;
  }
  document.documentElement.dataset.theme = choice;
}

function ThemeToggle() {
  const [choice, setChoice] = useState<ThemeChoice>(readTheme);

  useEffect(() => {
    applyTheme(choice);
    try {
      if (choice === "auto") {
        localStorage.removeItem(themeKey);
      } else {
        localStorage.setItem(themeKey, choice);
      }
    } catch {
      // The choice still applies for this visit when storage is blocked.
    }
  }, [choice]);

  function flip(): void {
    const mediaDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    const showing = choice === "auto" ? (mediaDark ? "dark" : "light") : choice;
    setChoice(showing === "light" ? "dark" : "light");
  }

  return (
    <button className="theme-toggle" type="button" aria-label="Switch light or dark theme" onClick={flip}>
      ◐
    </button>
  );
}

function SupportLine() {
  return (
    <div className="nav-meta">
      <ThemeToggle />
      <p className="nav-support">
        questions? <a href="mailto:support@cadri.io">support@cadri.io</a>
      </p>
    </div>
  );
}
