import { useEffect, type ReactNode } from "react";

export function Page({
  title,
  actions,
  children,
  fill = false,
}: {
  title: string;
  actions?: ReactNode;
  children?: ReactNode;
  fill?: boolean;
}) {
  useEffect(() => {
    document.title = title === "Memories" ? "Cadri" : `${title} — Cadri`;
  }, [title]);

  return (
    <section className={fill ? "page page-fill" : "page"}>
      <div className="page-head">
        <h1>{title}</h1>
        {actions}
      </div>
      {children}
    </section>
  );
}
