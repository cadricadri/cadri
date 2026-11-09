import { useState, type ReactNode } from "react";
import { Link } from "react-router";
import { AuthorMark } from "../memories/AuthorMark";
import { HarnessLogo, harnessInfo } from "./HarnessLogo";
import { commitLink, formatDuration, formatTime, type SessionDetail } from "./types";

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </>
  );
}

function CopyId({ id }: { id: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <span className="copy-id">
      <code title={id}>{id}</code>
      <button
        type="button"
        className="tree-mini"
        onClick={() => {
          void navigator.clipboard?.writeText(id).then(() => {
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1500);
          });
        }}
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </span>
  );
}

export function SessionSidebar({ detail, userLabel }: { detail: SessionDetail; userLabel: string | null }) {
  const tools = Object.entries(detail.tool_counts).sort(([, left], [, right]) => right - left);
  return (
    <aside className="session-side" aria-label="Session details">
      <section className="side-card">
        <h2>Overview</h2>
        <dl className="side-facts">
          <Fact label="Harness">
            <span className="side-harness">
              <HarnessLogo harness={detail.harness} size={16} />
              {harnessInfo(detail.harness).label}
              {detail.harness_version ? <span className="side-mute"> {detail.harness_version}</span> : null}
            </span>
          </Fact>
          {detail.model ? <Fact label="Model">{detail.model}</Fact> : null}
          {userLabel ? <Fact label="User">{userLabel}</Fact> : null}
          {detail.cwd ? (
            <Fact label="Folder">
              <code className="side-path" title={detail.cwd}>
                {detail.cwd}
              </code>
            </Fact>
          ) : null}
          <Fact label="Start">{formatTime(detail.started_at)}</Fact>
          <Fact label="End">{formatTime(detail.ended_at)}</Fact>
          <Fact label="Duration">{formatDuration(detail.started_at, detail.ended_at)}</Fact>
          <Fact label="Session">
            <CopyId id={detail.session_id} />
          </Fact>
        </dl>
      </section>

      <section className="side-card">
        <h2>Memories generated</h2>
        {detail.memory_commits.length === 0 ? (
          <p className="side-mute">No memory commits name this session.</p>
        ) : (
          <ul className="side-memories">
            {detail.memory_commits.map((commit) => (
              <li key={`${commit.book}:${commit.id}`}>
                <Link to={commitLink(commit)}>{commit.message || commit.id.slice(0, 8)}</Link>
                <span className="side-memory-meta">
                  <AuthorMark author={commit.author} />
                  <span>{commit.book === "user" ? "Personal" : "Team"}</span>
                  <time dateTime={commit.at}>{formatTime(commit.at)}</time>
                </span>
                {commit.files.length > 0 ? <code className="side-files">{commit.files.join(", ")}</code> : null}
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="side-card">
        <h2>
          Tool calls <span className="side-mute">{detail.tool_call_count}</span>
        </h2>
        {tools.length === 0 ? (
          <p className="side-mute">No tool calls.</p>
        ) : (
          <ul className="side-tools">
            {tools.map(([name, count]) => (
              <li key={name}>
                <code>{name}</code>
                <span className="side-tool-bar">
                  <span style={{ width: `${(count / tools[0][1]) * 100}%` }} />
                </span>
                <span className="side-tool-count">{count}</span>
              </li>
            ))}
          </ul>
        )}
        {detail.has_writes ? <p className="side-mute">This session edited files.</p> : null}
      </section>

      <section className="side-card">
        <h2>Activity</h2>
        <dl className="side-facts">
          <Fact label="Your messages">{detail.user_turns}</Fact>
          <Fact label="Agent replies">{detail.assistant_turns}</Fact>
          <Fact label="Events">{detail.event_count}</Fact>
          <Fact label="Size">{(detail.bytes / 1024).toFixed(1)} KiB</Fact>
          <Fact label="Last upload">{formatTime(detail.updated_at)}</Fact>
        </dl>
      </section>
    </aside>
  );
}
