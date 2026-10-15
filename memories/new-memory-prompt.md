Write a memory of the conversation for someone who will continue the work later.

Persist the story of what the user was trying to understand, what was learned, what was decided, and what was built. A reader who was not in the conversation should be able to follow why each piece exists.

Write prose, in the order the work actually unfolded (e.g. during an investigation when chasing down a root cause) or a hypothesis driven research conversation). When building new components, group by architectural understanding and component layout when appropriate (e.g. when building a multi-step architecture, one heading could be Frontend WebUI, another could be Backend Engine, and key decisions can be grouped by architectural area). Or for example, when building a CI/CD pipeline, mentioning layout of how changes flow through the system and how various subcomponents and steps are ran. One paragraph per stretch of understanding. Use a short list when listing key, important technologies used. When a command or approach was corrected, say what was tried, why it failed, and what works, in the same place.

Keep:
- The question the user was answering and the conclusion they reached.
- The mental model of the system (what is the client, what is the server, what each integration piece is for).
- Decisions that change how the system is built or run, including the command that actually works.
- What was built, in plain language: what it does, not file-level trivia.

Do not use the headings Product & Environment, User Prompt and Issue, Steps Tried & Results, Identifiers, Timeline Milestones, Tool Calls and Skills Used Insights, Current Status & Blockers, or Remaining Work.

If a fact was not established in the conversation, leave it out. 
