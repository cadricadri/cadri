import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { SessionCanvas, SessionPlan } from "./types";

/** A design or canvas as it sat in the conversation: open at first, foldable after that. */
function DocumentCard({
  title,
  overview,
  body,
  index,
  kicker,
  idPrefix,
}: {
  title: string;
  overview?: string;
  body: string;
  index: number;
  kicker: string;
  idPrefix: string;
}) {
  return (
    <li className="chat-plan">
      <details id={`${idPrefix}-${index}`} className="plan-doc" open>
        <summary>
          <span className="plan-kicker">{kicker}</span>
          <span className="plan-summary-title">{title}</span>
        </summary>
        <div className="plan-body">
          {overview ? <p className="plan-overview">{overview}</p> : null}
          <div className="memory-preview chat-markdown">
            <Markdown remarkPlugins={[remarkGfm]}>{withoutRepeatedTitle(title, body)}</Markdown>
          </div>
        </div>
      </details>
    </li>
  );
}

export function PlanCard({ plan, index }: { plan: SessionPlan; index: number }) {
  return (
    <DocumentCard
      title={plan.title}
      overview={plan.overview}
      body={plan.body}
      index={index}
      kicker="Plan"
      idPrefix="plan"
    />
  );
}

export function CanvasCard({ canvas, index }: { canvas: SessionCanvas; index: number }) {
  return <DocumentCard title={canvas.title} body={canvas.body} index={index} kicker="Canvas" idPrefix="canvas" />;
}

function withoutRepeatedTitle(title: string, body: string): string {
  const lines = body.split("\n");
  if (lines[0]?.trim().toLowerCase() === `# ${title}`.toLowerCase()) {
    return lines.slice(1).join("\n").trim();
  }
  return body;
}
