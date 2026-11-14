import { useState } from "react";

export const DOCS_URL = "https://cadri.io/docs";
export const INSTALL_DOCS_URL = `${DOCS_URL}/installation`;
export const JOIN_DOCS_URL = `${DOCS_URL}/joining`;
export const SEED_DOCS_URL = `${INSTALL_DOCS_URL}#4-seed-memories-agent`;
export const INSTALL_PROMPT = `Follow the installation instructions at ${INSTALL_DOCS_URL}

Seeding memory usually takes several minutes. The default window is whichever is smaller: the last 60 days, or the 100 most recent sessions. To use a smaller window, add a line like: only seed from the last 10 days or 10 sessions, whichever is smaller.`;

export const JOIN_PROMPT = `Follow the instructions at ${JOIN_DOCS_URL}

You are joining a team that already has a memory book. Revise that shared book from your sessions. This usually takes several minutes, and longer when the book and the session window are both large. The default window is whichever is smaller: the last 60 days, or the 100 most recent sessions. To use a smaller window, add a line like: only read the last 10 days or 10 sessions, whichever is smaller.`;

export function CopyButton({ text, label = "Copy prompt" }: { text: string; label?: string }) {
  const [copied, setCopied] = useState(false);

  return (
    <button
      className="btn"
      type="button"
      onClick={() => {
        void navigator.clipboard.writeText(text).then(
          () => {
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1500);
          },
          () => undefined,
        );
      }}
    >
      {copied ? "Copied" : label}
    </button>
  );
}
