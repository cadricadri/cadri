import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { CopyButton, DOCS_URL, INSTALL_DOCS_URL, INSTALL_PROMPT, SEED_DOCS_URL } from "../../install";

export type EmptyPhase = "install" | "seeding" | "empty";

type MemoryScope = "team" | "user";

export function EmptyBook({ scope, phase }: { scope: MemoryScope; phase: EmptyPhase }) {
  return (
    <div className="memory-file">
      <div className="memory-slug">
        <p className="memory-crumb">
          <span aria-current="page">INITIALIZING.md</span>
        </p>
      </div>
      <section className="memory-preview memory-onboarding" aria-label="INITIALIZING.md">
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{explainer(scope, phase)}</ReactMarkdown>
        <pre className="install-prompt">{INSTALL_PROMPT}</pre>
        <div className="form-actions">
          <CopyButton text={INSTALL_PROMPT} />
          {links(phase).map((link) => (
            <a key={link.href} className="btn ghost" href={link.href} target="_blank" rel="noreferrer">
              {link.label}
            </a>
          ))}
        </div>
      </section>
    </div>
  );
}

function explainer(scope: MemoryScope, phase: EmptyPhase): string {
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

function links(phase: EmptyPhase): { href: string; label: string }[] {
  const install = { href: INSTALL_DOCS_URL, label: "Open install docs" };
  if (phase === "install") {
    return [install];
  }
  return [install, { href: SEED_DOCS_URL, label: "How seeding works" }, { href: DOCS_URL, label: "Docs" }];
}
