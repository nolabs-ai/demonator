# Auto-Advance and Recording

Auto-advance mode removes the need to press Enter between steps. The demo runs
hands-free with configurable pauses, making it perfect for recording.

## Enabling auto-advance

Set `auto_advance` at the global level to specify how many milliseconds to wait
after each command finishes before advancing:

```yaml
auto_advance: 1500

steps:
  - text: "echo 'step one'"
  - text: "echo 'step two'"
  - text: "echo 'step three'"
```

## Per-step overrides

Override the global auto-advance delay on individual steps with `wait`:

```yaml
auto_advance: 1000

steps:
  - text: "echo 'quick pause'"
    wait: 500

  - text: "echo 'this output is important, let it breathe'"
    wait: 3000

  - text: "echo 'back to normal'"
```

## Recording with asciinema

Auto-advance pairs perfectly with [asciinema](https://asciinema.org) for
creating shareable terminal recordings:

```sh
asciinema rec -c "demonator -c recording.yml" demo.cast
```

Example config for a recording:

```yaml
speed: 25
auto_advance: 1500
clear: true
highlight: true

steps:
  - comment: "Building the project..."
  - text: "cargo build --release"
    wait: 2000
  - comment: "Running tests..."
  - text: "cargo test"
    wait: 2000
  - comment: "All done!"
```

## Converting to GIF

Use [agg](https://github.com/asciinema/agg) to convert the recording to a GIF
for embedding in READMEs or blog posts:

```sh
agg demo.cast demo.gif
```

## Tips

- Use `wait: 0` on a step to advance immediately (no pause at all)
- Comments use the configured typewriter timing; add a `wait` on the following
  command if readers need more time after a comment has finished typing
- The `pause` directive respects `auto_advance` — it waits the global delay
  instead of waiting for Enter
- Chapter navigation is not available in auto-advance mode (there's no Enter
  prompt to type into)
