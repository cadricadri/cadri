import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  CopyButton,
  DOCS_URL,
  INSTALL_DOCS_URL,
  INSTALL_PROMPT,
  JOIN_DOCS_URL,
  JOIN_PROMPT,
  SEED_DOCS_URL,
} from "../../install";

export type EmptyPhase = "install" | "seeding" | "empty";
export type EmptyFlow = "install" | "join";

type MemoryScope = "team" | "user";

export function EmptyBook({
  scope,
  phase,
  flow = "install",
}: {
  scope: MemoryScope;
  phase: EmptyPhase;
  flow?: EmptyFlow;
}) {
  const prompt = flow === "join" ? JOIN_PROMPT : INSTALL_PROMPT;
  return (
    <div className="memory-file">
      <div className="memory-slug">
        <p className="memory-crumb">
          <span aria-current="page">INITIALIZING.md</span>
        </p>
      </div>
      <section className="memory-preview memory-onboarding" aria-label="INITIALIZING.md">
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{explainer(scope, phase, flow)}</ReactMarkdown>
        <pre className="install-prompt">{prompt}</pre>
        <div className="form-actions">
          <CopyButton text={prompt} />
          {links(phase, flow).map((link) => (
            <a key={link.href} className="btn ghost" href={link.href} target="_blank" rel="noreferrer">
              {link.label}
            </a>
          ))}
        </div>
      </section>
    </div>
  );
}

function explainer(scope: MemoryScope, phase: EmptyPhase, flow: EmptyFlow): string {
  if (flow === "join") {
    return joinExplainer(phase);
  }
  if (phase === "install") {
    return [
      "# Connect your coding agents",
      "",
      "Cadri builds memories from your agents' sessions, and none have arrived yet.",
      "",
      "Paste this prompt into Claude Code, Cursor, Codex, or OpenCode. Your agent installs Cadri, signs this machine in, uploads recent sessions, and then writes your first memories. Seeding usually takes several minutes.",
    ].join("\n");
  }
  if (phase === "seeding") {
    return [
      "# Memories are being populated",
      "",
      "Your agent is reading recent sessions and writing the lessons worth keeping into your team and personal memories. This usually takes several minutes, and longer when there are many sessions. This page refreshes on its own.",
      "",
      "Haven't installed Cadri on this machine yet? Paste this prompt into your agent.",
    ].join("\n");
  }
  const title = scope === "user" ? "No personal memories yet" : "No team memories yet";
  return [
    `# ${title}`,
    "",
    "Seeding may still be running, or it finished without a lesson worth keeping. Ask your agent to load the `cadri-memory-seed` skill if this stays empty. You can also upload a Markdown file from the file list.",
    "",
    "Paste this prompt into your agent if Cadri isn't installed on this machine yet.",
  ].join("\n");
}

function joinExplainer(phase: EmptyPhase): string {
  if (phase === "install") {
    return [
      "# Join your team's memory",
      "",
      "This team already has a memory book. Your sessions are not here yet.",
      "",
      "Paste this prompt into Claude Code, Cursor, Codex, or OpenCode. Your agent installs Cadri, signs this machine in, uploads your recent sessions, and revises the shared book from what those sessions know. Your personal book starts empty. This usually takes several minutes.",
    ].join("\n");
  }
  if (phase === "seeding") {
    return [
      "# Your sessions are being folded into the team book",
      "",
      "Your agent is reading the memories your team already has and revising them from your sessions. Your personal book is written in the same pass. This usually takes several minutes, and longer when the book is large. This page refreshes on its own.",
      "",
      "Haven't installed Cadri on this machine yet? Paste this prompt into your agent.",
    ].join("\n");
  }
  return [
    "# No personal memories yet",
    "",
    "The team book already has memories. Ask your agent to load the `cadri-memory-join` skill if your personal book stays empty. You can also upload a Markdown file from the file list.",
    "",
    "Paste this prompt into your agent if Cadri isn't installed on this machine yet.",
  ].join("\n");
}

function links(phase: EmptyPhase, flow: EmptyFlow): { href: string; label: string }[] {
  const install = {
    href: flow === "join" ? JOIN_DOCS_URL : INSTALL_DOCS_URL,
    label: flow === "join" ? "How joining works" : "Open install docs",
  };
  if (phase === "install") {
    return [install];
  }
  if (flow === "join") {
    return [install, { href: DOCS_URL, label: "Docs" }];
  }
  return [install, { href: SEED_DOCS_URL, label: "How seeding works" }, { href: DOCS_URL, label: "Docs" }];
}
