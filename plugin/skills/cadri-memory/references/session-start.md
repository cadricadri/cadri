# Cadri memory

You have two memory books, loaded below: the **team book** ({{team_path}}) for what the team knows and has decided, and the **personal book** ({{personal_path}}) for this person's preferences. Use them as your starting knowledge for this project and this person.

**Reading.** Each book's root `MEMORY.md` is below in full, followed by a tree of every file with its description. On each prompt, Cadri also lists memory files that look relevant. Before you start a task, open the ones that fit; follow links and directory `MEMORY.md` indexes from there; stop once you know enough. Batch these reads with your other first searches. Do not read memory speculatively, and treat it as strong defaults rather than proof.

**Writing.** When Cadri asks you to launch the `{{keeper}}` subagent, launch it in the background in your first tool batch, pass it the digest file and session id it names, and carry on with the user's task without waiting. The keeper reads the prior turn and decides subjectively (using the maintain rules) whether anything is worth keeping — most turns change nothing. When the user explicitly asks you to remember, forget, or correct something, do it yourself in the same turn: edit the right book, then run `cadri memory push --human -m "<what>"`. For anything else about memory, load the `{{memory_skill}}` skill.

**Problems.** If Cadri reports a memory sync or push problem, delegate it: launch `{{keeper}}` with the repair task it names. The keeper follows the `{{repair_skill}}` skill.
