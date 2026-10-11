# geniewish

A voice interface for working on your Mac.

Early development. The application core is written in Rust, with a small native Mac voice preview.

## Mac voice preview

Requires Rust 1.93+ and Xcode Command Line Tools. Install the Codex CLI and sign in, or use the runtime bundled with the ChatGPT Mac app. Quit any text client using the same state directory first, then run:

```sh
./scripts/run-mac.sh
```

Click **Start listening** and allow macOS microphone and speech recognition permissions. Use headphones: this preview keeps listening while a genie speaks, so it can hear interruptions and switches. Speaker echo cancellation is not implemented.

Speak naturally and pause for about a second to send the phrase. Microphone capture stays on between phrases; there is no need to press Stop. Recognition retries automatically after recoverable errors.

An LLM interprets every voice or text input and chooses its destination. Routing receives the current roster/status and recent reply context, without adding work requests to HR's conversation. Uncertain destinations get a short clarification. The HR manager has its own lifetime conversation and uses geniewish's tools to hire, retire, update roles, check messages, and delegate work. Rust validates names, lifetime identities, approvals, and retirement confirmations. Pure switches run immediately after interpretation.

Examples:

```text
Hi Mike, please hire a new genie
I want to speak to Lucas
Help me plan a web page
Talk to Mike
Mike, any messages?
Read Lucas's message
```

You can combine actions: 'Mike, hire a product manager and ask them to draft a homepage plan.' Mike picks an available name; it may differ from Lucas on an existing team. Spoken replies use each genie's assigned installed Mac voice. Background results play a quiet chime. Asking a genie to stop talking interrupts playback. The Pause listening button is an optional microphone privacy control; Start listening resumes it. Neither command cancels work. A stopped result remains unread.

The app uses on-device English recognition and refuses cloud fallback. Audio is not saved. Conversation text stays with the work runtime. The window shows this session's exchanges; the same saved lifetime conversations continue across restarts.

It uses the repository's existing `.geniewish` state. To choose another workspace or an explicit state directory:

```sh
./scripts/run-mac.sh --workspace /path/to/project --state-dir /path/to/team
```

This is an early voice preview. Real microphone accuracy, pronunciation, echo, and timing need user trials. The examples are not required keywords. Interpretation adds a model call per phrase; wording and speech errors may still need clarification. Diagnostics never activate the microphone:

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

The text client remains a supported fallback. It and the native window's text field use the same Rust command handling as recognized speech. `cargo test` verifies routing, lifetime conversations, and delivery without a microphone. `--self-test` checks speech endpointing separately; actual recognition and interruption quality require microphone trials. The runtime protocol tests use canned model decisions and tool requests to test identity preservation, failure handling, delegation, retirement consent, and delivery acknowledgements; they do not evaluate model quality. Separate live text trials exercise the real router and HR model. The local MCP adapter is embedded in the Rust executable and configured only on HR's thread, including when resuming an older saved conversation; it never edits global runtime configuration.

For live work, install the Codex CLI and sign in, then run:

```sh
cargo run -- --check-runtime
cargo run -- --live --workspace /path/to/your/project
```

Enter one command per line. Natural input requires `--live`. Start by choosing the HR manager's name:

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

Final replies are plain prose and brief by default. Detailed reports go to `~/Desktop/geniewish`; the genie announces the filename after saving. Project source stays in its workspace. Use `--output-dir PATH` for another report folder. The HR manager has management and report tools; work genies keep their native project tools.

Routing, HR, and work genies use your configured runtime model by default. Optional `--router-model MODEL`, `--hr-model MODEL`, and `--worker-model MODEL` select them independently in both clients. No smaller model is selected automatically; availability depends on your signed-in account.

Roles are optional and stay blank until assigned. The manager picks an available name when none is given; a chosen name must be in the catalog. Roles can be updated or cleared later (`Mike, clear Allan's role`) in the same lifetime conversation.

Switching resumes the same conversation. A follow-up to a busy genie steers its active work and receives a short acknowledgement. Results arrive after the turn completes.

The user is called `Human`. Local metadata is saved in `.geniewish/`; use `--state-dir` to choose a different location. Conversation history stays with the work runtime. Retiring deletes the genie's conversation, while project files remain.

The app-server protocol is experimental. The current integration was checked with `codex-cli 0.162.0-alpha.17.2`; upgrades need compatibility checks.
