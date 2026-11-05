// cadri-managed-opencode-plugin: written by `cadri init`, removed by `cadri uninstall`.
// Hands each finished turn's session id to `cadri hook opencode`, which uploads the session.

const COMMAND: string[] = __CADRI_COMMAND__

function send(sessionID: string, event: string, directory: string) {
  try {
    const proc = Bun.spawn(COMMAND, { stdin: "pipe", stdout: "ignore", stderr: "ignore" })
    proc.stdin.write(JSON.stringify({ session_id: sessionID, hook_event_name: event, cwd: directory }))
    proc.stdin.end()
    proc.unref()
  } catch {
    // Capture must never break the agent.
  }
}

export const CadriPlugin = async ({ directory }: { directory: string }) => ({
  event: async ({ event }: { event: { type?: string; properties?: Record<string, any> } }) => {
    const properties = event?.properties ?? {}
    if (event?.type === "session.created") {
      const id = properties.info?.id ?? properties.sessionID
      if (typeof id === "string") send(id, "session.created", directory)
    } else if (event?.type === "session.idle") {
      const id = properties.sessionID
      if (typeof id === "string") send(id, "session.idle", directory)
    }
  },
})
