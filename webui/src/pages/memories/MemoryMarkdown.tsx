import { createContext, useContext, type ComponentProps } from "react";
import ReactMarkdown from "react-markdown";
import { Link, useSearchParams } from "react-router";
import remarkGfm from "remark-gfm";
import { memoryFileLocation } from "./links";

const remarkPlugins = [remarkGfm];
const MemoryPathContext = createContext("");

type AnchorProps = ComponentProps<"a"> & { node?: unknown };

function MemoryAnchor({ href, children, node: _node, ...props }: AnchorProps) {
  const currentPath = useContext(MemoryPathContext);
  const [params] = useSearchParams();
  const to = memoryFileLocation(currentPath, href, params);
  if (!to) {
    return (
      <a href={href} {...props}>
        {children}
      </a>
    );
  }
  return (
    <Link to={to} {...props}>
      {children}
    </Link>
  );
}

const components = { a: MemoryAnchor };

export function MemoryMarkdown({ path, children }: { path: string; children: string }) {
  return (
    <MemoryPathContext.Provider value={path}>
      <ReactMarkdown remarkPlugins={remarkPlugins} components={components}>
        {children}
      </ReactMarkdown>
    </MemoryPathContext.Provider>
  );
}
