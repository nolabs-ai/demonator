# Setup and Teardown

Setup and teardown commands run silently before and after the demo. The audience
never sees them. Use these to prepare the environment and clean up afterward.

## Basic usage

```yaml
setup:
  - "docker-compose down -v 2>/dev/null"
  - "rm -rf /tmp/demo-workspace"
  - "mkdir -p /tmp/demo-workspace"

teardown:
  - "docker-compose down -v"
  - "rm -rf /tmp/demo-workspace"

steps:
  - text: "ls /tmp/demo-workspace"
  - text: "echo 'clean environment!'"
```

## How it works

- **Setup** commands run sequentially before any steps are displayed
- **Teardown** commands run sequentially after all steps complete
- Both run with stdin, stdout, and stderr redirected to `/dev/null`
- Both inherit any global [`env:`](environment.md) values you have set
- If a setup or teardown command fails, a warning is printed to stderr but
  execution continues

## Use cases

### Cleaning up from a previous demo run

```yaml
setup:
  - "rm -rf myproject"
  - "docker stop demo-db 2>/dev/null"
  - "docker rm demo-db 2>/dev/null"
```

### Seeding a database

```yaml
setup:
  - "docker-compose up -d postgres"
  - "sleep 3"
  - "psql -h localhost -U demo -f seed.sql"

teardown:
  - "docker-compose down -v"
```

### Creating temporary files

```yaml
setup:
  - "mkdir -p /tmp/demo"
  - "echo '{\"key\": \"value\"}' > /tmp/demo/config.json"

teardown:
  - "rm -rf /tmp/demo"
```

## Tips

- Setup runs even in `--dry-run` mode (so your environment is ready if you then
  do a real run), but teardown does not run in dry-run mode
- If a setup command needs time to complete (like starting a database), add a
  `sleep` in the setup list
- Redirect stderr in setup commands to suppress expected warnings:
  `"docker stop foo 2>/dev/null"`
- Teardown runs even if the demo is interrupted partway through (as long as
  demonator exits normally — Ctrl+C between commands will skip teardown).
  On macOS and Linux, Ctrl+C during a command interrupts that command and lets
  the demo continue, including its teardown.
