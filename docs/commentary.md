# Commentary and Narration

Commentary blocks let you add explanatory text between commands. They appear
at the shell prompt and are not executed — they're purely for narration. Prefix
the text with `#` when you want it to look like a shell comment.

## Basic usage

```yaml
steps:
  - comment: "# First, let's clone the repository and build the project."
  - text: "git clone git@github.com:example/myproject.git"
  - text: "cd myproject"

  - comment: "# Now we'll run the test suite to make sure everything passes."
  - text: "cargo test"

  - comment: "# All green! Let's build for release."
  - text: "cargo build --release"
```

Comments are rendered in **dim** text by default, visually separating them from
the commands and their output. They use the same typewriter timing as commands,
including the global `speed` (or `delay`), `jitter`, and punctuation `pause`
settings.

## Timing

Comments inherit the global timing settings, or can override them individually:

```yaml
speed: 25
jitter: 20
pause: 150

steps:
  - comment: "This comment uses the global timing."

  - comment: "This comment is typed more quickly."
    speed: 50
    jitter: 5
    pause: 50
```

As with commands, `speed` is measured in characters per second and takes
precedence over `delay` when both apply.

## Styling

Use the `style` field to change the appearance:

```yaml
- comment: "This is dim (default)"

- comment: "This is bold"
  style: bold

- comment: "This is italic"
  style: italic

- comment: "This is in yellow"
  style: yellow
```

Available styles:

| Style      | Effect                    |
|------------|---------------------------|
| `dim`      | Dimmed text (default)     |
| `bold`     | Bold text                 |
| `italic`   | Italic text               |
| `red`      | Red text                  |
| `green`    | Green text                |
| `yellow`   | Yellow text               |
| `blue`     | Blue text                 |
| `magenta`  | Magenta text              |
| `cyan`     | Cyan text                 |

## Tips

- Use comments to explain **why** you're running a command, not just what it does
- Keep comments short — the audience is watching a live demo, not reading docs
- Combine with the `pause` directive if you want to give the audience time to read:

```yaml
- comment: "This next part deploys to production. Watch closely."
- pause: 2000
- text: "kubectl apply -f deploy.yml"
```

Timed pauses are measured in milliseconds. The next prompt appears before the
timer starts, so the following comment types into an already-visible prompt.
Use bare `- pause` instead when you want to wait for Enter.
