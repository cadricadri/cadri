import "./style.css";
import { type Auth, type AuthContext, fetchAuthMode, noAuth, tokenAuth } from "./auth";

type Transcript = {
  session_id: string;
  cwd: string | null;
  hook_event: string | null;
  content: string;
  created_at: string;
  updated_at: string;
  bytes: number;
  preview: string;
};

const status = document.querySelector<HTMLParagraphElement>("#status");
const rows = document.querySelector<HTMLTableSectionElement>("#rows");
const refresh = document.querySelector<HTMLButtonElement>("#refresh");
const actions = document.querySelector<HTMLDivElement>("#actions");

let auth: Auth = noAuth;

function cell(text: string): HTMLTableCellElement {
  const td = document.createElement("td");
  td.textContent = text;
  return td;
}

function formatTranscript(content: string): string {
  return content
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => {
      try {
        return JSON.stringify(JSON.parse(line), null, 2);
      } catch {
        return line;
      }
    })
    .join("\n\n");
}

function render(transcripts: Transcript[]): void {
  if (!rows || !status) {
    return;
  }
  rows.replaceChildren();
  if (transcripts.length === 0) {
    status.textContent = "No transcripts yet.";
    return;
  }
  status.textContent = `${transcripts.length} transcript${transcripts.length === 1 ? "" : "s"}.`;
  for (const transcript of transcripts) {
    const tr = document.createElement("tr");
    tr.append(
      cell(transcript.session_id),
      cell(transcript.updated_at),
      cell(transcript.cwd ?? ""),
      cell(transcript.hook_event ?? ""),
      cell(String(transcript.bytes)),
    );
    const transcriptCell = document.createElement("td");
    const pre = document.createElement("pre");
    pre.className = "transcript";
    pre.textContent = formatTranscript(transcript.content);
    transcriptCell.append(pre);
    tr.append(transcriptCell);
    rows.append(tr);
  }
}

function show(message: string): void {
  rows?.replaceChildren();
  if (status) {
    status.textContent = message;
  }
}

async function load(): Promise<void> {
  if (!status || !auth.ready()) {
    return;
  }
  status.textContent = "Loading…";
  try {
    const response = await fetch("/api/transcripts", { headers: auth.headers() });
    if (response.status === 401) {
      auth.unauthorized();
      return;
    }
    if (!response.ok) {
      throw new Error(`${response.status} ${response.statusText}`);
    }
    const transcripts = (await response.json()) as Transcript[];
    render(transcripts);
  } catch (error) {
    status.textContent =
      error instanceof Error ? `Could not load transcripts: ${error.message}` : "Could not load transcripts.";
  }
}

async function setupAuth(ctx: AuthContext): Promise<Auth> {
  const mode = await fetchAuthMode();
  if (mode === "token") {
    return tokenAuth(ctx);
  }
  if (mode === "workos") {
    const { workosAuth } = await import("./ee/workos");
    return workosAuth(ctx);
  }
  return noAuth;
}

async function start(): Promise<void> {
  if (actions) {
    auth = await setupAuth({ actions, show, reload: () => void load() });
  }
  refresh?.addEventListener("click", () => {
    void load();
  });
  await load();
}

void start();
