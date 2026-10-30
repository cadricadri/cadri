import { useEffect, type ReactNode } from "react";

export function Page({
  title,
  actions,
  beside,
  children,
  fill = false,
}: {
  title: string;
  actions?: ReactNode;
  beside?: ReactNode;
  children?: ReactNode;
  fill?: boolean;
}) {
  useEffect(() => {
    document.title = title === "Memories" ? "Cadri" : `${title} — Cadri`;
  }, [title]);

  return (
    <section className={fill ? "page page-fill" : "page"}>
      <div className="page-head">
        <div className="page-title">
          <h1>{title}</h1>
          {beside}
        </div>
        {actions}
      </div>
      {children}
    </section>
  );
}
