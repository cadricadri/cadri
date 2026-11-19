import { Fragment } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { CanvasCard, PlanCard } from "./Plans";
import { formatTime, type ChatMessage, type SessionCanvas, type SessionPlan } from "./types";

export function ChatThread({
  messages,
  plans = [],
  canvases = [],
  speaker = "You",
}: {
  messages: ChatMessage[];
  plans?: SessionPlan[];
  canvases?: SessionCanvas[];
  speaker?: string;
}) {
  if (messages.length === 0 && plans.length === 0 && canvases.length === 0) {
    return <p className="memory-empty">This session has no messages Cadri could read.</p>;
  }
  const after = cardsByMessage(plans, messages.length);
  const canvasAfter = cardsByMessage(canvases, messages.length);
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
          {(canvasAfter.get(index) ?? []).map((canvasIndex) => (
            <CanvasCard key={`canvas-${canvasIndex}`} canvas={canvases[canvasIndex]} index={canvasIndex} />
          ))}
        </Fragment>
      ))}
      {(after.get(messages.length) ?? []).map((planIndex) => (
        <PlanCard key={`plan-${planIndex}`} plan={plans[planIndex]} index={planIndex} />
      ))}
      {(canvasAfter.get(messages.length) ?? []).map((canvasIndex) => (
        <CanvasCard key={`canvas-${canvasIndex}`} canvas={canvases[canvasIndex]} index={canvasIndex} />
      ))}
    </ol>
  );
}

/** Cards land after the message that wrote them. Ones past the end of the thread go last. */
function cardsByMessage(cards: { after_message?: number }[], count: number): Map<number, number[]> {
  const grouped = new Map<number, number[]>();
  cards.forEach((card, index) => {
    const at = card.after_message ?? count;
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
