# Working on geniewish

- Keep the application small. Reuse existing integrations before adding infrastructure.
- Implement the core in Rust and target macOS first.
- Use lowercase `geniewish` in user-facing text.
- Each genie has one conversation for its entire lifetime. Switching resumes it.
- Never silently replace a saved conversation when reconnecting fails.
- Use an LLM to interpret natural input before sending anything to a genie conversation. Keep routing context small; validate the structured decision in Rust.
- Clarify uncertain intent or names; never silently fall back to sending an unparsed input as work.
- HR uses the local management tools on its lifetime runtime thread. The input router only chooses a destination or immediate app control; it cannot execute team changes.
- Bind management callbacks to the active native HR tool item; preserve named retirement consent and actual tool results.
- Switching speakers does not cancel background work.
- Roles are optional. An omitted role stays blank; role edits preserve the identity and thread.
- The manager selects an available catalog name when Human omits one.
- Keep the text client as a fallback. Voice and text must use the same Rust command and state logic.
- Test routing/lifecycle through text without a microphone; test speech capture/playback separately.
- Capture stays on between utterances; detect an end from a short pause and renew recognition independently.
- Keep credentials, local state, transcripts, private design notes, and strategic rationale out of this public repository and its Git history.
- Do not change the user's global Git, authentication, or runtime configuration.
- Run formatting, relevant tests, and lint checks for behavioral changes.
- Commit and push at meaningful, verified checkpoints. Do not commit runtime artifacts.

- Keep spoken results plain and brief. Detailed reports belong in the scoped output folder, with readable filenames; project source stays in its workspace.
