# geniewish

A voice interface for working on your Mac.

Early development. The application core is written in Rust. This first checkpoint is a text-driven client; microphone input and the Mac interface are still being developed.

## Development

```sh
cargo test
cargo run -- --help
```

Requires Rust 1.93 or newer. Protocol tests also use `python3`.

For live work, install the Codex CLI and sign in, then run:

```sh
cargo run -- --check-runtime
cargo run -- --live --workspace /path/to/your/project
```

Enter one command per line. Start by choosing the HR manager's name:

```text
Mike
Mike, hire Allan as my engineer
Allan, help me plan the web page
Talk to Mike
I want to speak to Allan
For the web page we're working on, use cold colors as the style
Mike, any messages?
Read Allan's message
Mike, retire Allan
Yes, retire Allan
quit
```

Switching resumes the same conversation. A follow-up to a busy genie steers its active work and receives a short acknowledgement. Results arrive after the turn completes.

The user is called `Human`. Local metadata is saved in `.geniewish/`; use `--state-dir` to choose a different location. Conversation history stays with the work runtime. Retiring deletes the genie's conversation, while project files remain.

The app-server protocol is experimental. The current integration was checked with `codex-cli 0.162.0-alpha.17.2`; upgrades need compatibility checks.
