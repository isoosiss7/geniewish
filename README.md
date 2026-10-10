# geniewish

A voice interface for working on your Mac.

Early development. The application core is written in Rust, with a small native Mac voice preview.

## Mac voice preview

Requires Rust 1.93+ and Xcode Command Line Tools. Install the Codex CLI and sign in, or use the runtime bundled with the ChatGPT Mac app. Quit any text client using the same state directory first, then run:

```sh
./scripts/run-mac.sh
```

Click **Start listening** and allow macOS microphone and speech recognition permissions. Use headphones: this preview keeps listening while a genie speaks, so it can hear interruptions and switches. Speaker echo cancellation is not implemented.

Say a command and pause for about a second to send it:

```text
Hi Mike, please hire a new genie
I want to speak to Lucas
Help me plan a web page
Talk to Mike
Mike, any messages?
Read Lucas's message
```

Mike picks an available name; it may differ from Lucas on an existing team. Spoken replies use each genie's assigned installed Mac voice. Background results play a quiet chime. Saying **stop talking** interrupts playback; **pause listening** turns off the microphone until you click Start listening again. Neither command cancels work. A stopped result remains unread.

The app uses on-device English recognition and refuses cloud fallback. Audio is not saved. Conversation text stays with the work runtime. The window shows this session's exchanges; the same saved lifetime conversations continue across restarts.

It uses the repository's existing `.geniewish` state. To choose another workspace or an explicit state directory:

```sh
./scripts/run-mac.sh --workspace /path/to/project --state-dir /path/to/team
```

This is an early voice preview. Real microphone accuracy, pronunciation, echo, and timing need user trials. Natural commands currently cover the documented phrases; arbitrary wording may need rephrasing. Diagnostics never activate the microphone:

```sh
./scripts/run-mac.sh --diagnose
./scripts/run-mac.sh --self-test
```

## Troubleshooting

Logs are saved under `.geniewish/logs/` (or `--log-dir PATH`). In the Mac app, choose **geniewish → Open troubleshooting logs**, or press Command-L.

Both the Rust core and the Mac adapter write timestamped JSON lines. They record recognition lifecycle and errors, permissions, routing, runtime methods and failures, and playback. Each process keeps three files of up to about 1 MB, with ten recent sessions retained per component. Files have private permissions and are excluded from Git.

Transcripts and error text are omitted by default. For a local reproduction that includes recognized/typed commands:

```sh
./scripts/run-mac.sh --debug-transcripts
```

The text client supports the same flag. Debug transcripts may contain private instructions; keep these logs local. The app does not log audio recordings, authentication files, full model replies, or tool arguments.

## Development

```sh
cargo test
cargo run -- --help
```

Requires Rust 1.93 or newer. Protocol tests also use `python3`.

The text client remains a supported fallback. It and the native window's text field use the same Rust command handling as recognized speech. `cargo test` verifies routing, lifetime conversations, and delivery without a microphone. `--self-test` checks speech endpointing separately; actual recognition and interruption quality require microphone trials.

For live work, install the Codex CLI and sign in, then run:

```sh
cargo run -- --check-runtime
cargo run -- --live --workspace /path/to/your/project
```

Enter one command per line. Start by choosing the HR manager's name:

```text
Mike
Mike, hire a new genie
Mike, hire Allan
Mike, set Allan's role to my engineer
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

Roles are optional and stay blank until assigned. The manager picks an available name when none is given; a chosen name must be in the catalog. Roles can be updated or cleared later (`Mike, clear Allan's role`) in the same lifetime conversation.

Switching resumes the same conversation. A follow-up to a busy genie steers its active work and receives a short acknowledgement. Results arrive after the turn completes.

The user is called `Human`. Local metadata is saved in `.geniewish/`; use `--state-dir` to choose a different location. Conversation history stays with the work runtime. Retiring deletes the genie's conversation, while project files remain.

The app-server protocol is experimental. The current integration was checked with `codex-cli 0.162.0-alpha.17.2`; upgrades need compatibility checks.
