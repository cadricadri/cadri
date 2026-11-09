import { useState } from "react";

export const DOCS_URL = "https://cadri.io/docs";
export const INSTALL_DOCS_URL = `${DOCS_URL}/installation`;
export const SEED_DOCS_URL = `${INSTALL_DOCS_URL}#4-seed-memories-agent`;
export const INSTALL_PROMPT = `Follow the installation instructions at ${INSTALL_DOCS_URL}`;

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
