import { useEffect, type ReactNode } from "react";

export function Page({
  title,
  actions,
  children,
}: {
  title: string;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  useEffect(() => {
    document.title = title === "Memories" ? "Cadri" : `${title} — Cadri`;
  }, [title]);

  return (
    <section className="page">
      <div className="page-head">
        <h1>{title}</h1>
        {actions}
      </div>
      {children}
    </section>
  );
}
