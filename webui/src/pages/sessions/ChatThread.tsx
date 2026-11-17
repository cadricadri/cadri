import { Fragment } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { PlanCard } from "./Plans";
import { formatTime, type ChatMessage, type SessionPlan } from "./types";

export function ChatThread({
  messages,
  plans = [],
  speaker = "You",
}: {
  messages: ChatMessage[];
  plans?: SessionPlan[];
  speaker?: string;
}) {
  if (messages.length === 0 && plans.length === 0) {
    return <p className="memory-empty">This session has no messages Cadri could read.</p>;
  }
  const after = plansByMessage(plans, messages.length);
  return (
    <ol className="chat-thread">
      {messages.map((message, index) => (
        <Fragment key={index}>
          <li className={`chat-turn chat-${message.role}`}>
            <span className="chat-who">
              {message.role === "user" ? speaker : "Agent"}
              {message.at ? <time dateTime={message.at}>{formatTime(message.at)}</time> : null}
            </span>
            {message.notes?.length ? <WorkingNotes notes={message.notes} /> : null}
            {message.text ? (
              <div className="chat-bubble">
                {message.role === "user" ? (
                  <p className="chat-plain">{message.text}</p>
                ) : (
                  <div className="memory-preview chat-markdown">
                    <Markdown remarkPlugins={[remarkGfm]}>{message.text}</Markdown>
                  </div>
                )}
              </div>
            ) : null}
          </li>
          {(after.get(index) ?? []).map((planIndex) => (
            <PlanCard key={`plan-${planIndex}`} plan={plans[planIndex]} index={planIndex} />
          ))}
        </Fragment>
      ))}
      {(after.get(messages.length) ?? []).map((planIndex) => (
        <PlanCard key={`plan-${planIndex}`} plan={plans[planIndex]} index={planIndex} />
      ))}
    </ol>
  );
}

/** Plans land after the message that wrote them. Ones past the end of the thread go last. */
function plansByMessage(plans: SessionPlan[], count: number): Map<number, number[]> {
  const grouped = new Map<number, number[]>();
  plans.forEach((plan, index) => {
    const at = plan.after_message ?? count;
    const key = at >= count ? count : at;
    const bucket = grouped.get(key) ?? [];
    bucket.push(index);
    grouped.set(key, bucket);
  });
  return grouped;
}

function WorkingNotes({ notes }: { notes: string[] }) {
  return (
    <details className="chat-notes">
      <summary>
        {notes.length} working {notes.length === 1 ? "note" : "notes"}
      </summary>
      <ol>
        {notes.map((note, index) => (
          <li key={index} className="memory-preview chat-markdown">
            <Markdown remarkPlugins={[remarkGfm]}>{note}</Markdown>
          </li>
        ))}
      </ol>
    </details>
  );
}
