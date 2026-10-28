# demonator

Typewriter-style text display for terminal demos. Demonator reads a YAML config
file of commands and "types" them out character-by-character with realistic
timing, then executes each command when you press Enter — so live terminal demos
look natural and polished.

📖 **Full documentation: [nolabs-ai.github.io/demonator](https://nolabs-ai.github.io/demonator)**

## Install

```sh
cargo install demonator
```

Or build from source:

```sh
cargo install --path .
```

## Usage

```sh
demonator              # uses demo.yml in the current directory
demonator -c my.yml    # use a custom config file
demonator --dry-run    # preview demo flow without executing
demonator --watch      # re-run demo when config file changes
```

Press **Enter** after each step to run the command and advance to the next one.

On macOS and Linux, press **Ctrl+C** while a command is running to interrupt it
and continue the demo. Between commands (including while typing or waiting for
Enter), **Ctrl+C** exits Demonator.

## Quick start

Create `demo.yml`:

```yaml
speed: 20
clear: true
highlight: true

steps:
  - comment: "Let's build the project."
    style: cyan
  - text: "cargo build --release"
  - text: "cargo test"
  - text: "echo 'All done!'"
```

Run it:

```sh
demonator
```

## Features

| Feature | Description |
|---------|-------------|
| [Commentary](https://nolabs-ai.github.io/demonator/commentary/) | Styled narration text between commands |
| [Fake output](https://nolabs-ai.github.io/demonator/fake-output/) | Pre-defined command output for offline demos |
| [Syntax highlighting](https://nolabs-ai.github.io/demonator/syntax-highlighting/) | Colorize commands as they are typed |
| [Chapters](https://nolabs-ai.github.io/demonator/chapters/) | Named sections with keyboard navigation |
| [Auto-advance](https://nolabs-ai.github.io/demonator/auto-advance/) | Hands-free mode for recording with asciinema |
| [Wait-for-pattern](https://nolabs-ai.github.io/demonator/wait-for-pattern/) | Pause until regex matches in output |
| [Setup & teardown](https://nolabs-ai.github.io/demonator/setup-teardown/) | Hidden environment prep and cleanup |
| [Environment variables](https://nolabs-ai.github.io/demonator/environment/) | Global and per-step `env:` for commands |
| [Interactive commands](https://nolabs-ai.github.io/demonator/interactive/) | Expect-style responses to prompts |
| [User prompts](https://nolabs-ai.github.io/demonator/user-prompts/) | Collect yes/no or free-text input from the presenter |
| [Conditional steps](https://nolabs-ai.github.io/demonator/conditionals/) | Branch based on captured variables |
| [Capturing output](https://nolabs-ai.github.io/demonator/configuration/) | Extract values with regex and reuse them in later steps |

See the [configuration reference](https://nolabs-ai.github.io/demonator/configuration/)
for all global options, per-step overrides, and CLI flags.

## License

Apache-2.0
