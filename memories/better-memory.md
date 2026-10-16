User sought to understand how to build an coding agent harness for Cadri, in order to log and save memories in a memory database. They first looked at using the Vercel plugin as a reference implementation to understand how agents plugin with harnesses, and learned that coding harnesses allow integration via plugins. Using plugins, skills, commands, subagents, and hooks, users can write plugins for coding harnesses that modify their behavior to save memories. 

User sought to understand how plugins are actually built, curious about how they are authored and what languages they are built in. Web searches and research of Anthropic's top plugins on their marketplace found that markdown and JSON provide info for the harness to decide when to make calls to a back end. 

Developing that understanding further, we learned about an example implementation where we treat the coding agent harness as a client, where a skill will invoke MCP instructions to call a backend API. 

To understand when specific calls are made from the coding agent harness client, we looked at the hooks claude code provides, including SessionStart, Stop, PreCompact, SessionEnd, which would all be points at which to invoke the Cadri skill to persist memories. 

Next, the user instructed to use the above context to build a HelloWorld tracer bullet reference implementation that just stores agent user transcripts. User wanted a backend server in Rust, a WebUI with node and typescript. 

Next, we began building,

- on the harness side, we learned how to attach a user specificied plugin to Claude via `claude --plugin-dir ./plugin` (however, to note, we later learned this command did not work verbatim because the user is invoking claude via a local ollama inference (specific preference), and the commmand to launch plugin is instead `ollama launch claude --model gemma4:26b --yes -- --plugin-dir ./plugin -p "Reply with the single word pong."`)
- On the backend server side, we stood up a Rust axum server, that exposes a GET and POST API on transcripts
- On the front end web ui, we stood up a very simple single page application that calls that GET API to display a list of transcripts in a table