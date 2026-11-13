// cadri-managed-opencode-plugin: written by `cadri init`, removed by `cadri uninstall`.
// Gives OpenCode sessions Cadri's team and personal memory, and uploads each finished turn.
// Every call goes through `cadri hook <event> --harness opencode`; a failure there never
// reaches the agent.

import { spawn, spawnSync } from "node:child_process"
import { readdirSync, readFileSync } from "node:fs"
import { homedir } from "node:os"
import { join } from "node:path"

const expand = (path: string) => path.replace(/^\$HOME(?=\/|$)/, homedir())
const COMMAND: string[] = (__CADRI_COMMAND__ as string[]).map(expand)
const SKILLS_DIR: string = expand(__CADRI_SKILLS__)
const EDIT_TOOLS = new Set(["edit", "write", "patch", "multiedit", "apply_patch"])
const PRIMARY_AGENTS = new Set(["build", "plan"])
const V2_ENDS = new Set(["session.execution.succeeded", "session.execution.failed", "session.execution.interrupted"])

type Answer = { context?: string; notice?: string }

function args(event: string): string[] {
  return [...COMMAND.slice(1), "hook", event, "--harness", "opencode"]
}

// Waits for the answer; for session start and prompts, whose context the agent needs now.
function ask(event: string, payload: Record<string, unknown>, timeout: number): Answer {
  try {
    const result = spawnSync(COMMAND[0], args(event), { input: JSON.stringify(payload), timeout, encoding: "utf8" })
    return result.status === 0 && result.stdout ? JSON.parse(result.stdout) : {}
  } catch {
    return {}
  }
}

// Detached, so uploads and pushes survive `opencode run` tearing down its process group.
function tell(event: string, payload: Record<string, unknown>) {
  try {
    const child = spawn(COMMAND[0], args(event), { detached: true, stdio: ["pipe", "ignore", "ignore"] })
    child.on("error", () => undefined)
    child.stdin?.end(JSON.stringify(payload))
    child.unref()
  } catch {
    // Capture must never break the agent.
  }
}

function frontmatter(text: string): Record<string, string> {
  const match = /^---\n([\s\S]*?)\n---/.exec(text)
  const fields: Record<string, string> = {}
  for (const line of match?.[1].split("\n") ?? []) {
    const at = line.indexOf(":")
    if (at > 0) fields[line.slice(0, at).trim()] = line.slice(at + 1).trim().replace(/^["']|["']$/g, "")
  }
  return fields
}

function skills() {
  const found: { id: string; name: string; description: string; path: string; content: string }[] = []
  try {
    for (const name of readdirSync(SKILLS_DIR)) {
      const path = join(SKILLS_DIR, name, "SKILL.md")
      try {
        const content = readFileSync(path, "utf8")
        const fields = frontmatter(content)
        found.push({ id: fields.name || name, name: fields.name || name, description: fields.description || "", path, content })
      } catch {
        // Not a skill directory.
      }
    }
  } catch {
    // Skills are optional.
  }
  return found
}

function sessionOf(event: any): string | undefined {
  const id = event?.sessionID ?? event?.session?.id ?? event?.prompt?.sessionID
  return typeof id === "string" && id ? id : undefined
}

function lastReply(messages: readonly any[] | undefined): string | undefined {
  for (const message of [...(messages ?? [])].reverse()) {
    if ((message?.role ?? message?.info?.role) !== "assistant") continue
    const parts: any[] = message?.parts ?? message?.content ?? []
    const text = parts
      .map((part) => (typeof part === "string" ? part : part?.type === "text" ? part.text : ""))
      .filter(Boolean)
      .join("\n")
    if (text) return text
  }
  return undefined
}

function promptText(parts: readonly any[] | undefined): string {
  return (parts ?? [])
    .filter((part) => part?.type === "text" && !part.synthetic && typeof part.text === "string")
    .map((part) => part.text)
    .join("\n")
}

// The agent name Cadri should see: none for OpenCode's own primary agents.
function subagent(agent: unknown): string | undefined {
  return typeof agent === "string" && agent && !PRIMARY_AGENTS.has(agent) ? agent : undefined
}

function editedPath(args: any): string | undefined {
  const path = args?.filePath ?? args?.file_path ?? args?.path
  return typeof path === "string" ? path : undefined
}

// Session start, asked once per session: the first prompt or model call, whichever comes first.
function seeder(directory: string | undefined) {
  const seeds = new Map<string, string>()
  return (sessionID: string, agent?: string): string => {
    let seed = seeds.get(sessionID)
    if (seed === undefined) {
      seed = ask("session-start", { session_id: sessionID, cwd: directory, agent }, 3000).context ?? ""
      seeds.set(sessionID, seed)
    }
    return seed
  }
}

// OpenCode 1.x. Memory goes into the system prompt of every model call, so it survives
// compaction; each prompt's recall replaces the previous one.
async function server(input: any) {
  const directory = input?.directory
  const seed = seeder(directory)
  const recall = new Map<string, string>()
  // Only prompts name the agent, and they name primary agents too ("build"); a subagent is
  // an agent in a session with a parent. Edits, shells, and idle events reuse the answer.
  const agents = new Map<string, string | undefined>()
  const agentOf = (sessionID: string | undefined) => (sessionID ? agents.get(sessionID) : undefined)
  const learnAgent = async (sessionID: string, agent: unknown) => {
    if (agents.has(sessionID) || typeof agent !== "string" || !agent) return agentOf(sessionID)
    let parent: unknown
    try {
      const result = await input?.client?.session?.get({ path: { id: sessionID } })
      parent = (result?.data ?? result)?.parentID
    } catch {
      parent = subagent(agent)
    }
    agents.set(sessionID, parent ? agent : undefined)
    return agentOf(sessionID)
  }
  return {
    "experimental.chat.system.transform": async (event: any, output: { system: string[] }) => {
      const sessionID = sessionOf(event)
      if (!sessionID) return
      const text = seed(sessionID)
      if (text) output.system.push(text)
      const relevant = recall.get(sessionID)
      if (relevant) output.system.push(relevant)
    },
    "chat.message": async (event: any, output: { parts?: any[] }) => {
      const sessionID = sessionOf(event)
      if (!sessionID) return
      const agent = await learnAgent(sessionID, event?.agent)
      seed(sessionID, agent)
      const answer = ask("prompt", { session_id: sessionID, prompt: promptText(output?.parts), cwd: directory, agent }, 1500)
      if (answer.context) recall.set(sessionID, answer.context)
      else recall.delete(sessionID)
    },
    "tool.execute.after": async (event: any) => {
      const path = editedPath(event?.args)
      if (!EDIT_TOOLS.has(String(event?.tool ?? "").toLowerCase()) || !path) return
      const sessionID = sessionOf(event)
      tell("edit", { session_id: sessionID, paths: [path], cwd: directory, agent: agentOf(sessionID) })
    },
    // So `cadri memory push` run by the agent knows which session's edits are its own.
    "shell.env": async (event: any, output: { env: Record<string, string> }) => {
      const sessionID = sessionOf(event)
      if (!sessionID) return
      output.env.CADRI_SESSION_ID = sessionID
      output.env.CADRI_HARNESS = "opencode"
      const agent = agentOf(sessionID)
      if (agent) output.env.CADRI_AGENT = agent
    },
    "experimental.session.compacting": async (event: any) => {
      const sessionID = sessionOf(event)
      if (sessionID) tell("pre-compact", { session_id: sessionID })
    },
    event: async ({ event }: { event: { type?: string; properties?: Record<string, any> } }) => {
      const sessionID = event?.properties?.sessionID
      if (event?.type !== "session.idle" || typeof sessionID !== "string") return
      let reply: string | undefined
      try {
        const result = await input?.client?.session?.messages({ path: { id: sessionID } })
        reply = lastReply(result?.data ?? result)
      } catch {
        reply = undefined
      }
      tell("stop", { session_id: sessionID, last_message: reply, cwd: directory, agent: agentOf(sessionID) })
    },
  }
}

export default {
  id: "cadri.memory",
  server,
  async setup(ctx: any) {
    const directory = ctx?.location?.directory
    const seed = seeder(directory)
    const registrations: any[] = []
    const register = async (make: () => Promise<any>) => {
      try {
        registrations.push(await make())
      } catch {
        // An older OpenCode without this hook: carry on without it.
      }
    }

    const bundled = skills()
    await register(() =>
      ctx.skill.transform((editor: any) => {
        for (const skill of bundled) if (!editor.get(skill.id)) editor.add(skill)
      }),
    )

    // Session start: asked once per session, then added to every model call, so it
    // survives compaction the way a system prompt does.
    await register(() =>
      ctx.session.hook("context", (event: any) => {
        const sessionID = sessionOf(event)
        if (!sessionID) return
        const text = seed(sessionID, subagent(event?.agent))
        if (text) event.system.push({ type: "text", text })
      }),
    )

    await register(() =>
      ctx.session.hook("prompt", async (event: any) => {
        const sessionID = sessionOf(event)
        if (!sessionID) return
        seed(sessionID)
        const answer = ask("prompt", { session_id: sessionID, prompt: event?.prompt?.text, cwd: directory }, 1500)
        try {
          if (answer.context) {
            await ctx.session.instructions.entry.put({ sessionID, key: "cadri", value: answer.context })
          } else {
            await ctx.session.instructions.entry.remove({ sessionID, key: "cadri" })
          }
        } catch {
          // Instruction entries are experimental; recall is a nice-to-have.
        }
      }),
    )

    await register(() =>
      ctx.session.hook("compaction", (event: any) => {
        const sessionID = sessionOf(event)
        if (sessionID) tell("pre-compact", { session_id: sessionID })
      }),
    )

    await register(() =>
      ctx.tool.hook("execute.after", (event: any) => {
        const path = editedPath(event?.input)
        if (!EDIT_TOOLS.has(String(event?.tool ?? "").toLowerCase()) || !path) return
        tell("edit", { session_id: sessionOf(event), paths: [path], cwd: directory, agent: subagent(event?.agent) })
      }),
    )

    const controller = new AbortController()
    const consuming = (async () => {
      if (typeof ctx?.event?.subscribe !== "function") return
      for await (const event of ctx.event.subscribe({ signal: controller.signal })) {
        const sessionID = event?.data?.sessionID
        if (!V2_ENDS.has(event?.type) || typeof sessionID !== "string") continue
        let reply: string | undefined
        try {
          reply = lastReply(await ctx.session.context({ sessionID }))
        } catch {
          reply = undefined
        }
        tell("stop", { session_id: sessionID, last_message: reply, cwd: directory })
      }
    })().catch(() => undefined)

    return async () => {
      controller.abort()
      await consuming
      for (const registration of registrations) await registration?.dispose?.().catch?.(() => undefined)
    }
  },
}
