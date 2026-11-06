import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { formatTime, type ChatMessage } from "./types";

export function ChatThread({ messages }: { messages: ChatMessage[] }) {
  if (messages.length === 0) {
    return <p className="memory-empty">This transcript has no messages Cadri could read.</p>;
  }
  return (
    <ol className="chat-thread">
      {messages.map((message, index) => (
        <li key={index} className={`chat-turn chat-${message.role}`}>
          <span className="chat-who">
            {message.role === "user" ? "You" : "Agent"}
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
      ))}
    </ol>
  );
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
