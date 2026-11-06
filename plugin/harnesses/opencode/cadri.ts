// cadri-managed-opencode-plugin: written by `cadri init`, removed by `cadri uninstall`.
// Hands each finished turn's session id to `cadri hook opencode`, which uploads the session.

import { spawn } from "node:child_process"

const COMMAND: string[] = __CADRI_COMMAND__

function send(sessionID: unknown, event: string, directory: string) {
  if (typeof sessionID !== "string" || !sessionID) return
  try {
    // Detached so the upload survives `opencode run` tearing down its process group on exit.
    const child = spawn(COMMAND[0], COMMAND.slice(1), { detached: true, stdio: ["pipe", "ignore", "ignore"] })
    child.on("error", () => undefined)
    child.stdin?.end(JSON.stringify({ session_id: sessionID, hook_event_name: event, cwd: directory }))
    child.unref()
  } catch {
    // Capture must never break the agent.
  }
}

// OpenCode 1.x: plugin hooks with a V1 event bus.
async function server({ directory }: { directory: string }) {
  return {
    event: async ({ event }: { event: { type?: string; properties?: Record<string, any> } }) => {
      const properties = event?.properties ?? {}
      if (event?.type === "session.created") send(properties.info?.id ?? properties.sessionID, event.type, directory)
      if (event?.type === "session.idle") send(properties.sessionID, event.type, directory)
    },
  }
}

// OpenCode 2.x: setup() subscribes to the V2 event stream; the returned function cleans up.
// V2 has no session.created, so a session's first execution stands in for its start.
const V2_ENDS = new Set(["session.execution.succeeded", "session.execution.failed", "session.execution.interrupted"])

export default {
  id: "cadri.capture",
  server,
  async setup(ctx: any) {
    const directory = ctx?.location?.directory ?? process.cwd()
    const started = new Set<string>()
    const controller = new AbortController()
    const consuming = (async () => {
      if (typeof ctx?.event?.subscribe !== "function") return
      for await (const event of ctx.event.subscribe({ signal: controller.signal })) {
        const id = event?.data?.sessionID
        if (event?.type === "session.execution.started" && typeof id === "string" && !started.has(id)) {
          started.add(id)
          send(id, "session.created", directory)
        } else if (V2_ENDS.has(event?.type)) {
          send(id, event.type, directory)
        }
      }
    })().catch(() => undefined)
    return async () => {
      controller.abort()
      await consuming
    }
  },
}
