import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { SessionPlan } from "./types";

/** The plan as it sat in the conversation: open at first, foldable after that. */
export function PlanCard({ plan, index }: { plan: SessionPlan; index: number }) {
  return (
    <li className="chat-plan">
      <details id={`plan-${index}`} className="plan-doc" open>
        <summary>
          <span className="plan-kicker">Plan</span>
          <span className="plan-summary-title">{plan.title}</span>
        </summary>
        <div className="plan-body">
          {plan.overview ? <p className="plan-overview">{plan.overview}</p> : null}
          <div className="memory-preview chat-markdown">
            <Markdown remarkPlugins={[remarkGfm]}>{withoutRepeatedTitle(plan)}</Markdown>
          </div>
        </div>
      </details>
    </li>
  );
}

function withoutRepeatedTitle(plan: SessionPlan): string {
  const lines = plan.body.split("\n");
  if (lines[0]?.trim().toLowerCase() === `# ${plan.title}`.toLowerCase()) {
    return lines.slice(1).join("\n").trim();
  }
  return plan.body;
}
