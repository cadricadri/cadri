import type { ReactNode } from "react";

type Harness = {
  label: string;
  /** Bar color in the activity chart. */
  color: string;
  mark: ReactNode;
};

const HARNESSES: Record<string, Harness> = {
  claude_code: {
    label: "Claude Code",
    color: "#d97757",
    mark: (
      <g stroke="#d97757" strokeWidth="2.2" strokeLinecap="round">
        {Array.from({ length: 12 }, (_, index) => {
          const angle = (index * Math.PI) / 6;
          const inner = 2.2;
          const outer = index % 2 === 0 ? 8.6 : 6.8;
          return (
            <line
              key={index}
              x1={12 + Math.cos(angle) * inner}
              y1={12 + Math.sin(angle) * inner}
              x2={12 + Math.cos(angle) * outer}
              y2={12 + Math.sin(angle) * outer}
            />
          );
        })}
      </g>
    ),
  },
  cursor: {
    label: "Cursor",
    color: "var(--ink)",
    mark: (
      <g strokeLinejoin="round">
        <path d="M12 2.5 20.5 7.25v9.5L12 21.5 3.5 16.75v-9.5Z" fill="currentColor" />
        <path d="M12 12 20.5 7.25 12 2.5 3.5 7.25Z" fill="var(--paper)" opacity="0.28" />
        <path d="M12 12v9.5L3.5 16.75v-9.5Z" fill="var(--paper)" opacity="0.12" />
        <path d="M6.2 8.6 12 11.9l5.8-3.3" stroke="var(--paper)" strokeWidth="1.2" fill="none" />
      </g>
    ),
  },
  codex: {
    label: "Codex",
    color: "#10a37f",
    mark: (
      <g>
        <rect x="2.5" y="4" width="19" height="16" rx="3.5" fill="#10a37f" />
        <path d="m7 9.2 3 2.8-3 2.8" stroke="#fff" strokeWidth="1.8" fill="none" strokeLinecap="round" strokeLinejoin="round" />
        <path d="M12.4 15h4.6" stroke="#fff" strokeWidth="1.8" strokeLinecap="round" />
      </g>
    ),
  },
  opencode: {
    label: "OpenCode",
    color: "#8f7cf0",
    mark: (
      <g>
        <rect x="3.5" y="3.5" width="17" height="17" rx="1.5" fill="none" stroke="currentColor" strokeWidth="2.2" />
        <rect x="8" y="8" width="8" height="8" fill="#8f7cf0" />
      </g>
    ),
  },
};

export function harnessInfo(harness: string | undefined): Harness {
  const known = harness ? HARNESSES[harness] : undefined;
  if (known) {
    return known;
  }
  const label = harness ? harness.replace(/[_-]+/g, " ") : "Unknown";
  return {
    label,
    color: "var(--mute)",
    mark: (
      <text x="12" y="16.5" textAnchor="middle" fontSize="13" fontWeight="700" fill="currentColor">
        {label.slice(0, 1).toUpperCase()}
      </text>
    ),
  };
}

export function HarnessLogo({ harness, size = 22 }: { harness: string | undefined; size?: number }) {
  return (
    <svg className="harness-mark" viewBox="0 0 24 24" width={size} height={size} aria-hidden="true">
      {harnessInfo(harness).mark}
    </svg>
  );
}

/** The logo tile with the harness name under it, as in the session table. */
export function HarnessBadge({ harness }: { harness: string | undefined }) {
  return (
    <span className="harness-badge">
      <span className="harness-tile">
        <HarnessLogo harness={harness} />
      </span>
      <span className="harness-name">{harnessInfo(harness).label}</span>
    </span>
  );
}
