# Working on geniewish

- Keep the application small. Reuse existing integrations before adding infrastructure.
- Implement the core in Rust and target macOS first.
- Use lowercase `geniewish` in user-facing text.
- Each genie has one conversation for its entire lifetime. Switching resumes it.
- Never silently replace a saved conversation when reconnecting fails.
- Route a command before sending its text to a conversation.
- Switching speakers does not cancel background work.
- Keep credentials, local state, transcripts, private design notes, and strategic rationale out of this public repository and its Git history.
- Do not change the user's global Git, authentication, or runtime configuration.
- Run formatting, relevant tests, and lint checks for behavioral changes.
- Commit and push at meaningful, verified checkpoints. Do not commit runtime artifacts.
