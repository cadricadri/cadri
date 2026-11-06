export type DiffLine =
  | { kind: "same" | "add" | "del"; text: string; oldNo: number | null; newNo: number | null }
  | { kind: "skip"; count: number };

/** Above this many compared line pairs the diff gives up and shows a full replace. */
const MAX_CELLS = 4_000_000;
const CONTEXT = 3;

function splitLines(text: string | null): string[] {
  if (!text) {
    return [];
  }
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") {
    lines.pop();
  }
  return lines;
}

/** Line diff by longest common subsequence, after trimming the shared head and tail. */
export function diffLines(before: string | null, after: string | null): DiffLine[] {
  const left = splitLines(before);
  const right = splitLines(after);
  let head = 0;
  while (head < left.length && head < right.length && left[head] === right[head]) {
    head += 1;
  }
  let tail = 0;
  while (
    tail < left.length - head &&
    tail < right.length - head &&
    left[left.length - 1 - tail] === right[right.length - 1 - tail]
  ) {
    tail += 1;
  }
  const a = left.slice(head, left.length - tail);
  const b = right.slice(head, right.length - tail);
  const middle: Array<"same" | "add" | "del"> = [];
  if (a.length * b.length > MAX_CELLS) {
    middle.push(...a.map(() => "del" as const), ...b.map(() => "add" as const));
  } else {
    const width = b.length + 1;
    const lcs = new Uint32Array((a.length + 1) * width);
    for (let i = a.length - 1; i >= 0; i -= 1) {
      for (let j = b.length - 1; j >= 0; j -= 1) {
        lcs[i * width + j] =
          a[i] === b[j] ? lcs[(i + 1) * width + j + 1] + 1 : Math.max(lcs[(i + 1) * width + j], lcs[i * width + j + 1]);
      }
    }
    let i = 0;
    let j = 0;
    while (i < a.length || j < b.length) {
      if (i < a.length && j < b.length && a[i] === b[j]) {
        middle.push("same");
        i += 1;
        j += 1;
      } else if (i < a.length && (j === b.length || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])) {
        middle.push("del");
        i += 1;
      } else {
        middle.push("add");
        j += 1;
      }
    }
  }

  const lines: DiffLine[] = [];
  let oldNo = 1;
  let newNo = 1;
  const same = () => {
    lines.push({ kind: "same", text: left[oldNo - 1], oldNo, newNo });
    oldNo += 1;
    newNo += 1;
  };
  for (let index = 0; index < head; index += 1) {
    same();
  }
  for (const kind of middle) {
    if (kind === "same") {
      same();
    } else if (kind === "del") {
      lines.push({ kind, text: left[oldNo - 1], oldNo, newNo: null });
      oldNo += 1;
    } else {
      lines.push({ kind, text: right[newNo - 1], oldNo: null, newNo });
      newNo += 1;
    }
  }
  for (let index = 0; index < tail; index += 1) {
    same();
  }
  return collapse(lines);
}

/** Keeps `CONTEXT` unchanged lines around each change and folds the rest. */
function collapse(lines: DiffLine[]): DiffLine[] {
  const changed = lines.map((line) => line.kind === "add" || line.kind === "del");
  if (!changed.some(Boolean)) {
    return lines;
  }
  const keep = lines.map((_, index) => {
    for (let offset = -CONTEXT; offset <= CONTEXT; offset += 1) {
      if (changed[index + offset]) {
        return true;
      }
    }
    return false;
  });
  const out: DiffLine[] = [];
  let skipped = 0;
  lines.forEach((line, index) => {
    if (keep[index]) {
      if (skipped > 0) {
        out.push({ kind: "skip", count: skipped });
        skipped = 0;
      }
      out.push(line);
    } else {
      skipped += 1;
    }
  });
  if (skipped > 0) {
    out.push({ kind: "skip", count: skipped });
  }
  return out;
}

export function diffStats(lines: DiffLine[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const line of lines) {
    if (line.kind === "add") {
      added += 1;
    } else if (line.kind === "del") {
      removed += 1;
    }
  }
  return { added, removed };
}
