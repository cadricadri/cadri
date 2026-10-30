import type { ReactNode, SVGProps } from "react";

function Glyph(props: SVGProps<SVGSVGElement>) {
  return (
    <svg
      viewBox="0 0 16 16"
      width="15"
      height="15"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...props}
    />
  );
}

const glyphs: Record<string, ReactNode> = {
  memories: (
    <Glyph fill="currentColor" stroke="none">
      <path d="M8 1.4 9.15 4.9 12.7 6.05 9.15 7.2 8 10.7 6.85 7.2 3.3 6.05 6.85 4.9 8 1.4Z" />
      <path d="M12.15 10.2 12.6 11.5 13.9 11.95 12.6 12.4 12.15 13.7 11.7 12.4 10.4 11.95 11.7 11.5 12.15 10.2Z" />
    </Glyph>
  ),
  transcripts: (
    <Glyph>
      <path d="M4 2.2h5.4L12.8 5.5V13.8H4z" />
      <path d="M9.2 2.3V5.6h3.4" />
      <path d="M6 8.1h4.2M6 10.6h2.8" />
    </Glyph>
  ),
  usage: (
    <Glyph>
      <path d="M2.2 13.2h11.6" />
      <path d="M3.4 10.4 6 7.2l2.5 1.7L12.4 4" />
    </Glyph>
  ),
  team: (
    <Glyph>
      <circle cx="6" cy="5.4" r="1.7" />
      <circle cx="10.6" cy="5.9" r="1.35" />
      <path d="M2.7 12.6c.45-1.85 1.75-2.75 3.3-2.75s2.85.9 3.3 2.75" />
      <path d="M9.3 10c.75-.35 1.5-.45 2.2-.25 1.05.4 1.75 1.35 2.05 2.85" />
    </Glyph>
  ),
  docs: (
    <Glyph>
      <path d="M2.6 3.1h4.1A1.7 1.7 0 0 1 8 4.4V13a1.6 1.6 0 0 0-1.3-.45H2.6z" />
      <path d="M13.4 3.1H9.3A1.7 1.7 0 0 0 8 4.4V13c.4-.3.85-.45 1.3-.45h4.1z" />
    </Glyph>
  ),
  billing: (
    <Glyph>
      <rect x="1.6" y="3.4" width="12.8" height="9.2" rx="1.4" />
      <path d="M1.6 6.5h12.8" />
    </Glyph>
  ),
};

export function NavGlyph({ name }: { name: keyof typeof glyphs }) {
  return <span className="nav-icon">{glyphs[name]}</span>;
}

function PixelMark({ children }: { children: ReactNode }) {
  return (
    <svg className="author-icon" viewBox="0 0 18 18" width="18" height="18" aria-hidden="true" shapeRendering="crispEdges">
      {children}
    </svg>
  );
}

function blocks(items: Array<[number, number, number, number, string]>) {
  return items.map(([x, y, width, height, fill]) => (
    <rect key={`${x}-${y}-${width}-${fill}`} x={x} y={y} width={width} height={height} fill={fill} />
  ));
}

export function HumanMark() {
  const hat = "#ff9ed2";
  const brim = "#f06aaf";
  const skin = "#ffc7a3";
  const hair = "#2a2438";
  const shirt = "#6aa6ff";
  const sleeve = "#4d86e0";
  const pants = "#3c3a55";
  return (
    <PixelMark>
      {blocks([
        [6, 1, 6, 1, "#ffe4f4"],
        [5, 2, 8, 2, hat],
        [3, 4, 12, 1, brim],
        [4, 5, 2, 2, hair],
        [12, 5, 2, 2, hair],
        [6, 5, 6, 4, skin],
        [7, 6, 1, 1, "var(--ink)"],
        [10, 6, 1, 1, "var(--ink)"],
        [8, 8, 2, 1, "#e08a78"],
        [5, 9, 8, 4, shirt],
        [4, 10, 1, 2, sleeve],
        [13, 10, 1, 2, sleeve],
        [3, 11, 1, 1, skin],
        [14, 11, 1, 1, skin],
        [6, 13, 2, 3, pants],
        [10, 13, 2, 3, pants],
      ])}
    </PixelMark>
  );
}

export function RobotMark() {
  const body = "#4c93e0";
  const shade = "#2f6cb8";
  const eye = "#d9f6ff";
  const star = "#ffe27a";
  return (
    <PixelMark>
      {blocks([
        [8, 0, 2, 1, star],
        [7, 1, 4, 1, star],
        [8, 2, 2, 1, star],
        [8, 3, 2, 1, shade],
        [4, 4, 10, 6, body],
        [4, 4, 10, 1, shade],
        [6, 6, 2, 2, eye],
        [10, 6, 2, 2, eye],
        [6, 6, 1, 1, "#f4fdff"],
        [10, 6, 1, 1, "#f4fdff"],
        [7, 9, 4, 1, shade],
        [5, 10, 8, 4, body],
        [3, 11, 2, 2, shade],
        [13, 11, 2, 2, shade],
        [6, 14, 2, 2, shade],
        [10, 14, 2, 2, shade],
      ])}
    </PixelMark>
  );
}
