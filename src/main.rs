use clap::Parser;
use rand::Rng;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime};

static ENV_SNAPSHOT_COUNTER: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Configuration structures
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Config {
    #[serde(default = "default_prompt")]
    prompt: String,
    #[serde(default)]
    clear: bool,
    #[serde(default)]
    speed: Option<u64>,
    #[serde(default = "default_delay")]
    delay: u64,
    #[serde(default = "default_jitter")]
    jitter: u64,
    #[serde(default = "default_pause")]
    pause: u64,
    #[serde(default)]
    highlight: bool,
    #[serde(default)]
    auto_advance: Option<u64>,
    #[serde(default)]
    setup: Option<Vec<String>>,
    #[serde(default)]
    teardown: Option<Vec<String>>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    steps: Vec<Step>,
    #[serde(default)]
    chapters: Vec<Chapter>,
    #[serde(default = "default_true")]
    show_exit_status: bool,
}

#[derive(Deserialize)]
struct Chapter {
    name: String,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Step {
    Directive(String),
    Ask(AskStep),
    Input(InputStep),
    Command(CommandStep),
    Comment(CommentStep),
    TimedPause(TimedPauseStep),
}

#[derive(Deserialize)]
struct AskStep {
    ask: String,
    capture: String,
}

#[derive(Deserialize)]
struct InputStep {
    input: String,
    capture: String,
    #[serde(default)]
    default: Option<String>,
}

#[derive(Deserialize)]
struct TimedPauseStep {
    pause: u64,
}

#[derive(Deserialize)]
struct CommentStep {
    comment: String,
    #[serde(default)]
    style: Option<String>,
    #[serde(default)]
    speed: Option<u64>,
    #[serde(default)]
    delay: Option<u64>,
    #[serde(default)]
    jitter: Option<u64>,
    #[serde(default)]
    pause: Option<u64>,
}

#[derive(Deserialize)]
struct CommandStep {
    text: String,
    #[serde(default)]
    speed: Option<u64>,
    #[serde(default)]
    delay: Option<u64>,
    #[serde(default)]
    jitter: Option<u64>,
    #[serde(default)]
    pause: Option<u64>,
    #[serde(default)]
    capture: Option<Capture>,
    #[serde(default)]
    fake_output: Option<String>,
    #[serde(default)]
    output_speed: Option<u64>,
    #[serde(default = "default_true")]
    execute: bool,
    #[serde(default)]
    wait_for: Option<String>,
    #[serde(default = "default_timeout")]
    timeout: u64,
    #[serde(default)]
    wait: Option<u64>,
    #[serde(default)]
    interact: Option<Vec<Interaction>>,
    #[serde(default, rename = "if")]
    if_condition: Option<String>,
    #[serde(default)]
    unless: Option<String>,
    #[serde(default)]
    wait_before: bool,
    #[serde(default)]
    wait_after: bool,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Deserialize)]
struct Capture {
    name: String,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    json_path: Option<String>,
}

#[derive(Deserialize)]
struct Interaction {
    #[serde(default)]
    expect: Option<String>,
    send: String,
}

fn default_true() -> bool {
    true
}
fn default_timeout() -> u64 {
    30
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "demonator",
    about = "Typewriter-style text display for terminal demos"
)]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "demo.yml")]
    config: PathBuf,

    /// Preview demo flow without executing or animating
    #[arg(long)]
    dry_run: bool,

    /// Re-run demo when config file changes
    #[arg(long)]
    watch: bool,
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

fn default_prompt() -> String {
    "{green}~{reset} {blue}${reset} ".to_string()
}
fn default_delay() -> u64 {
    50
}
fn default_jitter() -> u64 {
    0
}
fn default_pause() -> u64 {
    0
}

// ---------------------------------------------------------------------------
// Color / highlighting
// ---------------------------------------------------------------------------

fn expand_colors(s: &str) -> String {
    s.replace("{black}", "\x1B[30m")
        .replace("{red}", "\x1B[31m")
        .replace("{green}", "\x1B[32m")
        .replace("{yellow}", "\x1B[33m")
        .replace("{blue}", "\x1B[34m")
        .replace("{magenta}", "\x1B[35m")
        .replace("{cyan}", "\x1B[36m")
        .replace("{white}", "\x1B[37m")
        .replace("{bold}", "\x1B[1m")
        .replace("{dim}", "\x1B[2m")
        .replace("{reset}", "\x1B[0m")
}

const HL_RESET: &str = "\x1B[0m";
const HL_BOLD_WHITE: &str = "\x1B[1;37m";
const HL_YELLOW: &str = "\x1B[33m";
const HL_GREEN: &str = "\x1B[32m";
const HL_CYAN: &str = "\x1B[36m";
const HL_MAGENTA: &str = "\x1B[35m";

fn highlight_command(text: &str) -> Vec<(char, &'static str)> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut result = Vec::with_capacity(len);
    let mut i = 0;
    let mut is_first_word = true;

    while i < len {
        let ch = chars[i];

        if ch == ' ' || ch == '\t' {
            result.push((ch, HL_RESET));
            i += 1;
            continue;
        }

        // Pipe, semicolon, ampersand — operators
        if ch == '|' || ch == ';' {
            result.push((ch, HL_CYAN));
            i += 1;
            is_first_word = true;
            continue;
        }
        if ch == '&' {
            result.push((ch, HL_CYAN));
            i += 1;
            if i < len && chars[i] == '&' {
                result.push(('&', HL_CYAN));
                i += 1;
            }
            is_first_word = true;
            continue;
        }

        // Redirects
        if ch == '>' || ch == '<' {
            result.push((ch, HL_CYAN));
            i += 1;
            if i < len && (chars[i] == '>' || chars[i] == '&') {
                result.push((chars[i], HL_CYAN));
                i += 1;
            }
            continue;
        }

        // Quoted strings
        if ch == '"' || ch == '\'' {
            let quote = ch;
            result.push((ch, HL_GREEN));
            i += 1;
            while i < len && chars[i] != quote {
                if chars[i] == '\\' && quote == '"' {
                    result.push((chars[i], HL_GREEN));
                    i += 1;
                    if i < len {
                        result.push((chars[i], HL_GREEN));
                        i += 1;
                    }
                } else {
                    result.push((chars[i], HL_GREEN));
                    i += 1;
                }
            }
            if i < len {
                result.push((chars[i], HL_GREEN));
                i += 1;
            }
            is_first_word = false;
            continue;
        }

        // Variables
        if ch == '$' {
            result.push((ch, HL_MAGENTA));
            i += 1;
            if i < len && chars[i] == '{' {
                while i < len {
                    let c = chars[i];
                    result.push((c, HL_MAGENTA));
                    i += 1;
                    if c == '}' {
                        break;
                    }
                }
            } else if i < len && chars[i] == '(' {
                result.push((chars[i], HL_MAGENTA));
                i += 1;
                // Subshell — just color the parens, let inner parse normally
            } else {
                while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    result.push((chars[i], HL_MAGENTA));
                    i += 1;
                }
            }
            is_first_word = false;
            continue;
        }

        // Flags: -x, --long-flag
        if ch == '-' && (i == 0 || chars[i - 1] == ' ' || chars[i - 1] == '\t') {
            while i < len && chars[i] != ' ' && chars[i] != '\t' && chars[i] != '=' {
                result.push((chars[i], HL_YELLOW));
                i += 1;
            }
            // Include = sign if present
            if i < len && chars[i] == '=' {
                result.push((chars[i], HL_YELLOW));
                i += 1;
            }
            is_first_word = false;
            continue;
        }

        // Comments in shell
        if ch == '#' && (i == 0 || chars[i - 1] == ' ') {
            while i < len {
                result.push((chars[i], HL_GREEN));
                i += 1;
            }
            continue;
        }

        // Regular word
        let color = if is_first_word {
            HL_BOLD_WHITE
        } else {
            HL_RESET
        };
        while i < len
            && !matches!(
                chars[i],
                ' ' | '\t' | '|' | ';' | '&' | '>' | '<' | '"' | '\'' | '$'
            )
        {
            result.push((chars[i], color));
            i += 1;
        }
        is_first_word = false;
    }

    result
}

// ---------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------

fn speed_to_delay(speed: u64) -> u64 {
    if speed == 0 {
        return default_delay();
    }
    ((1000.0 / speed as f64).round() as u64).max(1)
}

fn resolve_delay(cmd: &CommandStep, config: &Config) -> u64 {
    resolve_text_delay(cmd.speed, cmd.delay, config)
}

fn resolve_comment_delay(comment: &CommentStep, config: &Config) -> u64 {
    resolve_text_delay(comment.speed, comment.delay, config)
}

fn resolve_text_delay(speed: Option<u64>, delay: Option<u64>, config: &Config) -> u64 {
    if let Some(speed) = speed.or(config.speed) {
        speed_to_delay(speed)
    } else {
        delay.unwrap_or(config.delay)
    }
}

fn should_run_step(cmd: &CommandStep, vars: &HashMap<String, String>) -> bool {
    if let Some(ref var_name) = cmd.if_condition {
        match vars.get(var_name) {
            Some(v) if !v.trim().is_empty() => {}
            _ => return false,
        }
    }

    if let Some(ref var_name) = cmd.unless {
        if let Some(v) = vars.get(var_name) {
            if !v.trim().is_empty() {
                return false;
            }
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Text output
// ---------------------------------------------------------------------------

fn type_text(text: &str, delay: u64, jitter: u64, pause: u64) {
    let mut rng = rand::thread_rng();
    let stdout = io::stdout();
    let mut handle = stdout.lock();

    for ch in text.chars() {
        let base = delay as f64;
        let jitter_amount = if jitter > 0 {
            let j = (base * jitter as f64) / 100.0;
            rng.gen_range(-j..j)
        } else {
            0.0
        };

        let mut sleep_ms = (base + jitter_amount).max(5.0) as u64;
        if matches!(ch, '.' | ',' | ';' | ':' | '!' | '?') {
            sleep_ms += pause;
        }

        thread::sleep(Duration::from_millis(sleep_ms));
        handle.write_all(ch.to_string().as_bytes()).unwrap();
        handle.flush().unwrap();
    }

    handle.flush().unwrap();
}

fn type_text_highlighted(tokens: &[(char, &str)], delay: u64, jitter: u64, pause: u64) {
    let mut rng = rand::thread_rng();
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let mut current_color = "";

    for &(ch, color) in tokens {
        let base = delay as f64;
        let jitter_amount = if jitter > 0 {
            let j = (base * jitter as f64) / 100.0;
            rng.gen_range(-j..j)
        } else {
            0.0
        };

        let mut sleep_ms = (base + jitter_amount).max(5.0) as u64;
        if matches!(ch, '.' | ',' | ';' | ':' | '!' | '?') {
            sleep_ms += pause;
        }

        thread::sleep(Duration::from_millis(sleep_ms));

        if color != current_color {
            handle.write_all(color.as_bytes()).unwrap();
            current_color = color;
        }
        handle.write_all(ch.to_string().as_bytes()).unwrap();
        handle.flush().unwrap();
    }

    // Reset color at end
    handle.write_all(HL_RESET.as_bytes()).unwrap();
    handle.flush().unwrap();
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

enum NavAction {
    Continue,
    NextChapter,
    PrevChapter,
    JumpChapter(usize),
}

fn set_tty_mode(args: &[&str]) {
    if let Ok(f) = fs::File::open("/dev/tty") {
        let _ = Command::new("stty")
            .args(args)
            .stdin(Stdio::from(f))
            .status();
    }
}

fn restore_tty_mode(saved: Option<String>) {
    if let Some(saved) = saved {
        set_tty_mode(&[&saved]);
    }
}

fn drain_pending_enter_bytes(reader: &mut io::BufReader<fs::File>) {
    set_tty_mode(&["min", "0", "time", "1"]);

    let mut buf = [0u8; 1];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) if buf[0] == b'\n' || buf[0] == b'\r' => {}
            Ok(_) => break,
        }
    }
}

fn wait_for_input(has_chapters: bool) -> NavAction {
    let saved = fs::File::open("/dev/tty")
        .ok()
        .and_then(|f| {
            Command::new("stty")
                .arg("-g")
                .stdin(Stdio::from(f))
                .output()
                .ok()
        })
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());

    if saved.is_some() {
        set_tty_mode(&["-echo", "-icanon", "min", "1", "time", "0"]);
    }

    let tty = fs::File::open("/dev/tty").expect("failed to open /dev/tty");
    let mut reader = io::BufReader::new(tty);
    let mut buf = [0u8; 1];
    let mut input: Vec<u8> = Vec::new();

    loop {
        if reader.read(&mut buf).unwrap_or(0) > 0 {
            if buf[0] == b'\n' || buf[0] == b'\r' {
                drain_pending_enter_bytes(&mut reader);
                break;
            }
            input.push(buf[0]);
        }
    }

    restore_tty_mode(saved);

    if !has_chapters || input.is_empty() {
        return NavAction::Continue;
    }

    let trimmed = String::from_utf8_lossy(&input);
    match trimmed.as_ref() {
        "n" => NavAction::NextChapter,
        "p" => NavAction::PrevChapter,
        s => {
            if let Ok(num) = s.parse::<usize>() {
                if num > 0 {
                    NavAction::JumpChapter(num - 1)
                } else {
                    NavAction::Continue
                }
            } else {
                NavAction::Continue
            }
        }
    }
}

// Wait for any keypress without echoing (no cursor movement, safe for erasing
// the line afterwards). Used by wait_after so Enter's newline doesn't interfere.
fn wait_for_any_key_silent() {
    let saved = fs::File::open("/dev/tty")
        .ok()
        .and_then(|f| {
            Command::new("stty")
                .arg("-g")
                .stdin(Stdio::from(f))
                .output()
                .ok()
        })
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());

    if saved.is_some() {
        set_tty_mode(&["-echo", "-icanon", "min", "1", "time", "0"]);
    }

    let tty = fs::File::open("/dev/tty").expect("failed to open /dev/tty");
    let mut reader = io::BufReader::new(tty);
    let mut buf = [0u8; 1];
    // Accept any keypress — not just Enter — so no newline is generated.
    while reader.read(&mut buf).unwrap_or(0) == 0 {}
    if buf[0] == b'\n' || buf[0] == b'\r' {
        drain_pending_enter_bytes(&mut reader);
    }

    restore_tty_mode(saved);
}

// Wait for Enter without echoing the keystroke (keeps the cursor on the
// current line so the next command can be typed in place of the prompt).
fn wait_for_enter_silent() {
    let saved = fs::File::open("/dev/tty")
        .ok()
        .and_then(|f| {
            Command::new("stty")
                .arg("-g")
                .stdin(Stdio::from(f))
                .output()
                .ok()
        })
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());

    if saved.is_some() {
        set_tty_mode(&["-echo", "-icanon", "min", "1", "time", "0"]);
    }

    let tty = fs::File::open("/dev/tty").expect("failed to open /dev/tty");
    let mut reader = io::BufReader::new(tty);
    let mut buf = [0u8; 1];
    loop {
        if reader.read(&mut buf).unwrap_or(0) > 0 && (buf[0] == b'\n' || buf[0] == b'\r') {
            drain_pending_enter_bytes(&mut reader);
            break;
        }
    }

    restore_tty_mode(saved);
}

// ---------------------------------------------------------------------------
// Variable substitution
// ---------------------------------------------------------------------------

fn substitute_vars(text: &str, vars: &HashMap<String, String>) -> String {
    let mut result = text.to_string();
    for (name, value) in vars {
        result = result.replace(&format!("{{{}}}", name), value);
    }
    result
}

// ---------------------------------------------------------------------------
// JSON path extraction
// ---------------------------------------------------------------------------

/// Extract a value from JSON using a simple path syntax.
/// Supports dot notation and array indices: `[0].session_id`, `.name`, `foo.bar[2].baz`
fn extract_json_path(text: &str, path: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(text).ok()?;
    let mut current = &parsed;

    for segment in parse_json_path_segments(path) {
        match segment {
            JsonSegment::Key(key) => {
                current = current.get(&key)?;
            }
            JsonSegment::Index(idx) => {
                current = current.get(idx)?;
            }
        }
    }

    match current {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

enum JsonSegment {
    Key(String),
    Index(usize),
}

fn parse_json_path_segments(path: &str) -> Vec<JsonSegment> {
    let mut segments = Vec::new();
    let mut chars = path.chars().peekable();

    while chars.peek().is_some() {
        // skip leading dots
        if chars.peek() == Some(&'.') {
            chars.next();
        }

        if chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            let num: String = chars.by_ref().take_while(|c| *c != ']').collect();
            if let Ok(idx) = num.parse::<usize>() {
                segments.push(JsonSegment::Index(idx));
            }
        } else {
            let mut key = String::new();
            while let Some(&c) = chars.peek() {
                if c == '.' || c == '[' {
                    break;
                }
                key.push(c);
                chars.next();
            }
            if !key.is_empty() {
                segments.push(JsonSegment::Key(key));
            }
        }
    }

    segments
}

// ---------------------------------------------------------------------------
// Command execution
// ---------------------------------------------------------------------------

fn initial_env(config_env: &HashMap<String, String>) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in config_env {
        env.insert(k.clone(), v.clone());
    }
    env
}

fn merge_env(
    base: &HashMap<String, String>,
    overlay: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut env = base.clone();
    for (k, v) in overlay {
        env.insert(k.clone(), v.clone());
    }
    env
}

fn env_snapshot_path() -> PathBuf {
    let n = ENV_SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("demonator-env-{}-{}.env", process::id(), n))
}

fn shell_script_with_env_snapshot(cmd: &str) -> String {
    format!(
        "{}\n__demonator_status=$?\nenv -0 > \"$DEMONATOR_ENV_FILE\"\nexit \"$__demonator_status\"",
        cmd
    )
}

fn read_env_snapshot(path: &Path) -> Option<HashMap<String, String>> {
    let bytes = fs::read(path).ok()?;
    let _ = fs::remove_file(path);
    let mut env = HashMap::new();

    for entry in bytes.split(|b| *b == 0) {
        if entry.is_empty() {
            continue;
        }
        let Some(eq) = entry.iter().position(|b| *b == b'=') else {
            continue;
        };
        let key = String::from_utf8_lossy(&entry[..eq]).to_string();
        let value = String::from_utf8_lossy(&entry[eq + 1..]).to_string();
        env.insert(key, value);
    }

    Some(env)
}

fn persist_env_snapshot(
    env_state: &mut HashMap<String, String>,
    before: &HashMap<String, String>,
    overlay: &HashMap<String, String>,
    snapshot: Option<HashMap<String, String>>,
) {
    let Some(mut next) = snapshot else {
        return;
    };

    // Per-step `env:` values are temporary unless the command changes or unsets
    // them. Restore unchanged overlay keys so step-local overrides stay local.
    for (key, overlay_value) in overlay {
        if next.get(key) == Some(overlay_value) {
            if let Some(previous) = before.get(key) {
                next.insert(key.clone(), previous.clone());
            } else {
                next.remove(key);
            }
        }
    }

    *env_state = next;
}

fn shell_status_with_env_snapshot(
    cmd: &str,
    env: &HashMap<String, String>,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
) -> io::Result<(ExitStatus, Option<HashMap<String, String>>)> {
    let path = env_snapshot_path();
    let status = Command::new("sh")
        .arg("-c")
        .arg(shell_script_with_env_snapshot(cmd))
        .envs(env)
        .env("DEMONATOR_ENV_FILE", &path)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .status();
    let snapshot = read_env_snapshot(&path);
    status.map(|s| (s, snapshot))
}

fn shell_output_with_env_snapshot(
    cmd: &str,
    env: &HashMap<String, String>,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
) -> io::Result<(Output, Option<HashMap<String, String>>)> {
    let path = env_snapshot_path();
    let output = Command::new("sh")
        .arg("-c")
        .arg(shell_script_with_env_snapshot(cmd))
        .envs(env)
        .env("DEMONATOR_ENV_FILE", &path)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .output();
    let snapshot = read_env_snapshot(&path);
    output.map(|o| (o, snapshot))
}

fn run_command(
    cmd: &str,
    capture: Option<&Capture>,
    env_state: &mut HashMap<String, String>,
    overlay: &HashMap<String, String>,
    show_exit_status: bool,
) -> (Option<String>, i32) {
    let needs_capture = capture.is_some();
    let before = env_state.clone();
    let env = merge_env(env_state, overlay);

    if needs_capture {
        let output = shell_output_with_env_snapshot(
            cmd,
            &env,
            Stdio::inherit(),
            Stdio::piped(),
            Stdio::piped(),
        );

        match output {
            Ok((o, snapshot)) => {
                persist_env_snapshot(env_state, &before, overlay, snapshot);
                let stdout_str = String::from_utf8_lossy(&o.stdout);
                let stderr_str = String::from_utf8_lossy(&o.stderr);
                print!("{}", stdout_str);
                eprint!("{}", stderr_str);
                io::stdout().flush().unwrap();
                io::stderr().flush().unwrap();

                let code = o.status.code().unwrap_or(1);
                if !o.status.success() && show_exit_status {
                    eprintln!("[demonator] command exited with status {}", code);
                }

                if let Some(cap) = capture {
                    if let Some(ref jp) = cap.json_path {
                        if let Some(value) = extract_json_path(&stdout_str, jp) {
                            return (Some(value), code);
                        } else {
                            eprintln!("[demonator] json_path '{}' did not match", jp);
                        }
                    } else if let Some(ref pattern) = cap.pattern {
                        if let Ok(re) = Regex::new(pattern) {
                            let combined = format!("{}{}", stdout_str, stderr_str);
                            if let Some(caps) = re.captures(&combined) {
                                if let Some(m) = caps.get(1) {
                                    return (Some(m.as_str().to_string()), code);
                                }
                            }
                        } else {
                            eprintln!("[demonator] invalid capture pattern: {}", pattern);
                        }
                    }
                }

                (None, code)
            }
            Err(e) => {
                eprintln!("[demonator] failed to run command: {}", e);
                (None, 1)
            }
        }
    } else {
        let status = shell_status_with_env_snapshot(
            cmd,
            &env,
            Stdio::inherit(),
            Stdio::inherit(),
            Stdio::inherit(),
        );

        match status {
            Ok((s, snapshot)) => {
                persist_env_snapshot(env_state, &before, overlay, snapshot);
                let code = s.code().unwrap_or(1);
                if !s.success() && show_exit_status {
                    eprintln!("[demonator] command exited with status {}", code);
                }
                (None, code)
            }
            Err(e) => {
                eprintln!("[demonator] failed to run command: {}", e);
                (None, 1)
            }
        }
    }
}

fn run_command_wait_for(
    cmd: &str,
    pattern: &str,
    timeout_secs: u64,
    env_state: &mut HashMap<String, String>,
    overlay: &HashMap<String, String>,
) -> i32 {
    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[demonator] invalid wait_for pattern: {}", e);
            return 1;
        }
    };

    let before = env_state.clone();
    let env = merge_env(env_state, overlay);
    let path = env_snapshot_path();
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(shell_script_with_env_snapshot(cmd))
        .envs(&env)
        .env("DEMONATOR_ENV_FILE", &path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[demonator] failed to run command: {}", e);
            return 1;
        }
    };

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();

    let tx2 = tx.clone();
    thread::spawn(move || {
        let reader = io::BufReader::new(stdout);
        for line in reader.split(b'\n') {
            if let Ok(bytes) = line {
                let mut with_nl = bytes;
                with_nl.push(b'\n');
                if tx.send(with_nl).is_err() {
                    break;
                }
            }
        }
    });

    thread::spawn(move || {
        let reader = io::BufReader::new(stderr);
        for line in reader.split(b'\n') {
            if let Ok(bytes) = line {
                let mut with_nl = bytes;
                with_nl.push(b'\n');
                if tx2.send(with_nl).is_err() {
                    break;
                }
            }
        }
    });

    let deadline = SystemTime::now() + Duration::from_secs(timeout_secs);
    let mut accumulated = String::new();

    loop {
        let remaining = deadline
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            eprintln!("[demonator] wait_for timed out after {}s", timeout_secs);
            let _ = child.kill();
            let _ = child.wait();
            return 1;
        }

        match rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(chunk) => {
                let text = String::from_utf8_lossy(&chunk);
                print!("{}", text);
                io::stdout().flush().unwrap();
                accumulated.push_str(&text);

                if re.is_match(&accumulated) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return 0;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Check if child exited
                if let Ok(Some(status)) = child.try_wait() {
                    // Drain remaining output
                    for chunk in rx.try_iter() {
                        let text = String::from_utf8_lossy(&chunk);
                        print!("{}", text);
                        accumulated.push_str(&text);
                    }
                    io::stdout().flush().unwrap();

                    if re.is_match(&accumulated) {
                        persist_env_snapshot(env_state, &before, overlay, read_env_snapshot(&path));
                        return 0;
                    }
                    eprintln!("[demonator] command exited before pattern matched");
                    persist_env_snapshot(env_state, &before, overlay, read_env_snapshot(&path));
                    return status.code().unwrap_or(1);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = child.wait();
                persist_env_snapshot(env_state, &before, overlay, read_env_snapshot(&path));
                if re.is_match(&accumulated) {
                    return 0;
                }
                eprintln!("[demonator] command exited before pattern matched");
                return 1;
            }
        }
    }
}

fn run_command_interact(
    cmd: &str,
    interactions: &[Interaction],
    env_state: &mut HashMap<String, String>,
    overlay: &HashMap<String, String>,
) -> i32 {
    let before = env_state.clone();
    let env = merge_env(env_state, overlay);
    let path = env_snapshot_path();
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(shell_script_with_env_snapshot(cmd))
        .envs(&env)
        .env("DEMONATOR_ENV_FILE", &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[demonator] failed to run command: {}", e);
            return 1;
        }
    };

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();

    thread::spawn(move || {
        let mut reader = io::BufReader::new(stdout);
        let mut buf = [0u8; 256];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut accumulated = String::new();
    let mut interaction_idx = 0;

    while interaction_idx < interactions.len() {
        let interaction = &interactions[interaction_idx];

        if interaction.expect.is_none() {
            // No pattern to wait for — drain any pending output briefly then send.
            loop {
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(chunk) => {
                        print!("{}", String::from_utf8_lossy(&chunk));
                        io::stdout().flush().unwrap();
                    }
                    Err(_) => break,
                }
            }
            let response = format!("{}\n", interaction.send);
            if stdin.write_all(response.as_bytes()).is_err() {
                break;
            }
            let _ = stdin.flush();
            interaction_idx += 1;
            continue;
        }

        let expect = interaction.expect.as_deref().unwrap();
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(chunk) => {
                let text = String::from_utf8_lossy(&chunk);
                print!("{}", text);
                io::stdout().flush().unwrap();
                accumulated.push_str(&text);

                if accumulated.contains(expect) {
                    let response = format!("{}\n", interaction.send);
                    if stdin.write_all(response.as_bytes()).is_err() {
                        break;
                    }
                    let _ = stdin.flush();
                    accumulated.clear();
                    interaction_idx += 1;
                }
            }
            Err(_) => break,
        }
    }

    // Drain remaining output
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                print!("{}", String::from_utf8_lossy(&chunk));
            }
            Err(_) => break,
        }
    }
    io::stdout().flush().unwrap();

    let status = child.wait().unwrap_or_else(|_| process::exit(1));
    persist_env_snapshot(env_state, &before, overlay, read_env_snapshot(&path));
    status.code().unwrap_or(1)
}

// ---------------------------------------------------------------------------
// Feature helpers
// ---------------------------------------------------------------------------

fn style_to_ansi(style: Option<&str>) -> &str {
    match style {
        Some("dim") => "\x1B[2m",
        Some("bold") => "\x1B[1m",
        Some("italic") => "\x1B[3m",
        Some("red") => "\x1B[31m",
        Some("green") => "\x1B[32m",
        Some("yellow") => "\x1B[33m",
        Some("blue") => "\x1B[34m",
        Some("magenta") => "\x1B[35m",
        Some("cyan") => "\x1B[36m",
        _ => "\x1B[2m", // default to dim
    }
}

fn print_comment(comment: &CommentStep, config: &Config) {
    let ansi = style_to_ansi(comment.style.as_deref());
    print!("{}", ansi);
    io::stdout().flush().unwrap();
    type_text(
        &comment.comment,
        resolve_comment_delay(comment, config),
        comment.jitter.unwrap_or(config.jitter),
        comment.pause.unwrap_or(config.pause),
    );
    println!("\x1B[0m");
}

fn print_chapter_header(name: &str) {
    let bar = "─".repeat(name.len() + 4);
    println!("\x1B[1;36m┌{}┐\x1B[0m", bar);
    println!("\x1B[1;36m│  {}  │\x1B[0m", name);
    println!("\x1B[1;36m└{}┘\x1B[0m", bar);
    println!();
}

fn run_hidden_commands(commands: &[String], env_state: &mut HashMap<String, String>) {
    for cmd in commands {
        let before = env_state.clone();
        let status = shell_status_with_env_snapshot(
            cmd,
            env_state,
            Stdio::null(),
            Stdio::null(),
            Stdio::null(),
        );

        if let Ok((s, snapshot)) = status {
            persist_env_snapshot(env_state, &before, &HashMap::new(), snapshot);
            if !s.success() {
                eprintln!(
                    "[demonator] setup/teardown command failed ({}): {}",
                    s.code().unwrap_or(-1),
                    cmd
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Demo resolution — flatten chapters into indexed steps
// ---------------------------------------------------------------------------

struct ChapterMarker {
    name: String,
    start: usize,
}

struct ResolvedDemo {
    steps: Vec<ResolvedStep>,
    chapters: Vec<ChapterMarker>,
}

struct ResolvedStep {
    step: StepRef,
}

enum StepRef {
    Directive(String),
    TimedPause(u64),
    Comment(CommentRef),
    Ask(String, String),                   // (message, capture_name)
    Input(String, String, Option<String>), // (message, capture_name, default)
    Command(CommandRef),
}

struct CommentRef {
    comment: String,
    style: Option<String>,
    speed: Option<u64>,
    delay: Option<u64>,
    jitter: Option<u64>,
    pause: Option<u64>,
}

struct CommandRef {
    text: String,
    speed: Option<u64>,
    delay: Option<u64>,
    jitter: Option<u64>,
    pause: Option<u64>,
    capture: Option<CaptureRef>,
    fake_output: Option<String>,
    output_speed: Option<u64>,
    execute: bool,
    wait_for: Option<String>,
    timeout: u64,
    wait: Option<u64>,
    interact: Option<Vec<InteractionRef>>,
    if_condition: Option<String>,
    unless: Option<String>,
    wait_before: bool,
    wait_after: bool,
    env: HashMap<String, String>,
    hidden: bool,
}

struct CaptureRef {
    name: String,
    pattern: Option<String>,
    json_path: Option<String>,
}

struct InteractionRef {
    expect: Option<String>,
    send: String,
}

fn resolve_step(step: &Step) -> ResolvedStep {
    match step {
        Step::Directive(d) => ResolvedStep {
            step: StepRef::Directive(d.clone()),
        },
        Step::Ask(a) => ResolvedStep {
            step: StepRef::Ask(a.ask.clone(), a.capture.clone()),
        },
        Step::Input(i) => ResolvedStep {
            step: StepRef::Input(i.input.clone(), i.capture.clone(), i.default.clone()),
        },
        Step::TimedPause(p) => ResolvedStep {
            step: StepRef::TimedPause(p.pause),
        },
        Step::Comment(c) => ResolvedStep {
            step: StepRef::Comment(CommentRef {
                comment: c.comment.clone(),
                style: c.style.clone(),
                speed: c.speed,
                delay: c.delay,
                jitter: c.jitter,
                pause: c.pause,
            }),
        },
        Step::Command(cmd) => ResolvedStep {
            step: StepRef::Command(CommandRef {
                text: cmd.text.clone(),
                speed: cmd.speed,
                delay: cmd.delay,
                jitter: cmd.jitter,
                pause: cmd.pause,
                capture: cmd.capture.as_ref().map(|c| CaptureRef {
                    name: c.name.clone(),
                    pattern: c.pattern.clone(),
                    json_path: c.json_path.clone(),
                }),
                fake_output: cmd.fake_output.clone(),
                output_speed: cmd.output_speed,
                execute: cmd.execute,
                wait_for: cmd.wait_for.clone(),
                timeout: cmd.timeout,
                wait: cmd.wait,
                interact: cmd.interact.as_ref().map(|v| {
                    v.iter()
                        .map(|i| InteractionRef {
                            expect: i.expect.clone(),
                            send: i.send.clone(),
                        })
                        .collect()
                }),
                if_condition: cmd.if_condition.clone(),
                unless: cmd.unless.clone(),
                wait_before: cmd.wait_before,
                wait_after: cmd.wait_after,
                env: cmd.env.clone(),
                hidden: cmd.hidden,
            }),
        },
    }
}

fn resolve_demo(config: &Config) -> ResolvedDemo {
    if !config.chapters.is_empty() {
        let mut steps = Vec::new();
        let mut chapters = Vec::new();

        for ch in &config.chapters {
            chapters.push(ChapterMarker {
                name: ch.name.clone(),
                start: steps.len(),
            });
            for s in &ch.steps {
                steps.push(resolve_step(s));
            }
        }

        ResolvedDemo { steps, chapters }
    } else {
        let steps = config.steps.iter().map(resolve_step).collect();
        ResolvedDemo {
            steps,
            chapters: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Dry-run display
// ---------------------------------------------------------------------------

fn print_dry_run(config: &Config) {
    let demo = resolve_demo(config);
    let prompt = expand_colors(&config.prompt);

    if let Some(ref setup) = config.setup {
        println!("\x1B[2m[setup]\x1B[0m");
        for cmd in setup {
            println!("  {}", cmd);
        }
        println!();
    }

    let mut chapter_idx = 0;

    for (i, resolved) in demo.steps.iter().enumerate() {
        // Check chapter boundary
        if chapter_idx < demo.chapters.len() && demo.chapters[chapter_idx].start == i {
            println!("\x1B[1;36m── {} ──\x1B[0m", demo.chapters[chapter_idx].name);
            chapter_idx += 1;
        }

        match &resolved.step {
            StepRef::Directive(d) => {
                println!("\x1B[2m[{}]\x1B[0m", d);
            }
            StepRef::TimedPause(ms) => {
                println!("\x1B[2m[pause: {}ms]\x1B[0m", ms);
            }
            StepRef::Ask(msg, capture) => {
                println!("\x1B[33m[ask]\x1B[0m {} \x1B[2m→ {}\x1B[0m", msg, capture);
            }
            StepRef::Input(msg, capture, default) => {
                let default_hint = default
                    .as_deref()
                    .map(|d| format!(" \x1B[2m(default: {})\x1B[0m", d))
                    .unwrap_or_default();
                println!(
                    "\x1B[33m[input]\x1B[0m {} \x1B[2m→ {}{}\x1B[0m",
                    msg, capture, default_hint
                );
            }
            StepRef::Comment(comment) => {
                let ansi = style_to_ansi(comment.style.as_deref());
                println!("{}{}{}\x1B[0m", prompt, ansi, comment.comment);
            }
            StepRef::Command(cmd) => {
                print!("{}{}", prompt, cmd.text);
                let mut annotations = Vec::new();
                if !cmd.execute {
                    annotations.push("no-exec");
                }
                if cmd.fake_output.is_some() {
                    annotations.push("fake-output");
                }
                if cmd.wait_for.is_some() {
                    annotations.push("wait-for");
                }
                if cmd.interact.is_some() {
                    annotations.push("interactive");
                }
                if cmd.wait_after {
                    annotations.push("wait-after");
                }
                if cmd.if_condition.is_some() || cmd.unless.is_some() {
                    annotations.push("conditional");
                }
                if cmd.capture.is_some() {
                    annotations.push("capture");
                }
                if cmd.hidden {
                    annotations.push("hidden");
                }
                if !annotations.is_empty() {
                    print!("  \x1B[2m[{}]\x1B[0m", annotations.join(", "));
                }
                println!();

                if let Some(ref fake) = cmd.fake_output {
                    for line in fake.lines() {
                        println!("  \x1B[2m▸ {}\x1B[0m", line);
                    }
                }
            }
        }
    }

    if let Some(ref teardown) = config.teardown {
        println!();
        println!("\x1B[2m[teardown]\x1B[0m");
        for cmd in teardown {
            println!("  {}", cmd);
        }
    }
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

fn load_config(path: &Path) -> Config {
    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading {}: {}", path.display(), e);
            process::exit(1);
        }
    };

    let config: Config = match serde_yaml::from_str(&contents) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error parsing {}: {}", path.display(), e);
            process::exit(1);
        }
    };

    if config.steps.is_empty() && config.chapters.is_empty() {
        eprintln!("Error: config must have either 'steps' or 'chapters'");
        process::exit(1);
    }
    if !config.steps.is_empty() && !config.chapters.is_empty() {
        eprintln!("Error: config cannot have both 'steps' and 'chapters'");
        process::exit(1);
    }

    config
}

// ---------------------------------------------------------------------------
// Main demo loop
// ---------------------------------------------------------------------------

fn run_demo(config: &Config, cli: &Cli) {
    let mut runtime_env = initial_env(&config.env);

    // Setup
    if let Some(ref setup) = config.setup {
        run_hidden_commands(setup, &mut runtime_env);
    }

    // Dry run
    if cli.dry_run {
        print_dry_run(config);
        return;
    }

    // Clear
    if config.clear {
        print!("\x1B[2J\x1B[H");
        io::stdout().flush().unwrap();
    }

    let demo = resolve_demo(config);
    let has_chapters = !demo.chapters.is_empty();
    let prompt = expand_colors(&config.prompt);

    let mut vars: HashMap<String, String> = HashMap::new();
    let mut idx: usize = 0;
    let mut chapter_idx: usize = 0;
    // A `pause` leaves a prompt on screen and keeps the cursor on that line, so
    // the next command should type into it rather than print a second prompt.
    let mut prompt_shown = false;

    while idx < demo.steps.len() {
        // Check chapter boundary
        if chapter_idx < demo.chapters.len() && demo.chapters[chapter_idx].start == idx {
            print_chapter_header(&demo.chapters[chapter_idx].name);
            chapter_idx += 1;
        }

        match &demo.steps[idx].step {
            StepRef::Directive(directive) => match directive.as_str() {
                "pause" => {
                    print!("{}", prompt);
                    io::stdout().flush().unwrap();
                    if let Some(ms) = config.auto_advance {
                        thread::sleep(Duration::from_millis(ms));
                    } else {
                        wait_for_enter_silent();
                    }
                    prompt_shown = true;
                }
                "clear" => {
                    print!("\x1B[2J\x1B[H");
                    io::stdout().flush().unwrap();
                    prompt_shown = false;
                }
                other => {
                    eprintln!("[demonator] unknown directive: {}", other);
                    process::exit(1);
                }
            },

            StepRef::TimedPause(ms) => {
                if !prompt_shown {
                    print!("{}", prompt);
                    io::stdout().flush().unwrap();
                }
                prompt_shown = true;
                thread::sleep(Duration::from_millis(*ms));
            }

            StepRef::Ask(msg, capture_name) => {
                print!("\x1B[33m?\x1B[0m {} \x1B[2m[y/N]\x1B[0m ", msg);
                io::stdout().flush().unwrap();
                let tty = fs::File::open("/dev/tty").expect("failed to open /dev/tty");
                let mut reader = io::BufReader::new(tty);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap_or(0);
                let answer = line.trim().to_lowercase();
                if answer == "y" || answer == "yes" {
                    vars.insert(capture_name.clone(), "y".to_string());
                } else {
                    vars.remove(capture_name);
                }
                idx += 1;
                continue;
            }

            StepRef::Input(msg, capture_name, default) => {
                let hint = default.as_deref().unwrap_or("enter");
                print!("\x1B[33m?\x1B[0m {} \x1B[2m[{}]\x1B[0m ", msg, hint);
                io::stdout().flush().unwrap();
                let tty = fs::File::open("/dev/tty").expect("failed to open /dev/tty");
                let mut reader = io::BufReader::new(tty);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap_or(0);
                let answer = line.trim().to_string();
                let value = if answer.is_empty() {
                    default.clone()
                } else {
                    Some(answer)
                };
                if let Some(v) = value {
                    vars.insert(capture_name.clone(), v);
                } else {
                    vars.remove(capture_name);
                }
                idx += 1;
                continue;
            }

            StepRef::Comment(comment) => {
                if !prompt_shown {
                    print!("{}", prompt);
                    io::stdout().flush().unwrap();
                }
                prompt_shown = false;
                let comment = CommentStep {
                    comment: comment.comment.clone(),
                    style: comment.style.clone(),
                    speed: comment.speed,
                    delay: comment.delay,
                    jitter: comment.jitter,
                    pause: comment.pause,
                };
                print_comment(&comment, config);
            }

            StepRef::Command(cmd) => {
                // Evaluate conditionals
                let cmd_for_conditions = CommandStep {
                    text: cmd.text.clone(),
                    speed: cmd.speed,
                    delay: cmd.delay,
                    jitter: cmd.jitter,
                    pause: cmd.pause,
                    capture: None,
                    fake_output: cmd.fake_output.clone(),
                    output_speed: cmd.output_speed,
                    execute: cmd.execute,
                    wait_for: cmd.wait_for.clone(),
                    timeout: cmd.timeout,
                    wait: cmd.wait,
                    interact: None,
                    if_condition: cmd.if_condition.clone(),
                    unless: cmd.unless.clone(),
                    wait_before: cmd.wait_before,
                    wait_after: cmd.wait_after,
                    env: cmd.env.clone(),
                    hidden: cmd.hidden,
                };

                if !should_run_step(&cmd_for_conditions, &vars) {
                    idx += 1;
                    continue;
                }

                let resolved_text = substitute_vars(&cmd.text, &vars);

                if cmd.hidden {
                    // Run silently — no prompt, no typing, no output, no wait
                    let before_env = runtime_env.clone();
                    let step_env = merge_env(&runtime_env, &cmd.env);
                    if let Some(ref cap) = cmd.capture {
                        if let Ok((output, snapshot)) = shell_output_with_env_snapshot(
                            &resolved_text,
                            &step_env,
                            Stdio::null(),
                            Stdio::piped(),
                            Stdio::null(),
                        ) {
                            persist_env_snapshot(&mut runtime_env, &before_env, &cmd.env, snapshot);
                            let stdout_str = String::from_utf8_lossy(&output.stdout);
                            let value = if let Some(ref jp) = cap.json_path {
                                extract_json_path(&stdout_str, jp)
                            } else if let Some(ref pattern) = cap.pattern {
                                Regex::new(pattern).ok().and_then(|re| {
                                    re.captures(stdout_str.as_ref())
                                        .and_then(|caps| caps.get(1))
                                        .map(|m| m.as_str().to_string())
                                })
                            } else {
                                None
                            };
                            if let Some(v) = value {
                                vars.insert(cap.name.clone(), v.clone());
                                runtime_env.insert(cap.name.clone(), v);
                            }
                        }
                    } else {
                        if let Ok((_status, snapshot)) = shell_status_with_env_snapshot(
                            &resolved_text,
                            &step_env,
                            Stdio::null(),
                            Stdio::null(),
                            Stdio::null(),
                        ) {
                            persist_env_snapshot(&mut runtime_env, &before_env, &cmd.env, snapshot);
                        }
                    }
                    idx += 1;
                    continue;
                }

                // Type the command. A preceding `pause` already left a prompt on
                // this line, so don't print a second one.
                if !prompt_shown {
                    print!("{}", prompt);
                }
                prompt_shown = false;
                io::stdout().flush().unwrap();

                if cmd.wait_before {
                    wait_for_enter_silent();
                }

                let base_delay = resolve_delay(&cmd_for_conditions, config);
                let jitter_val = cmd.jitter.unwrap_or(config.jitter);
                let pause_val = cmd.pause.unwrap_or(config.pause);

                if config.highlight {
                    let tokens = highlight_command(&resolved_text);
                    type_text_highlighted(&tokens, base_delay, jitter_val, pause_val);
                } else {
                    type_text(&resolved_text, base_delay, jitter_val, pause_val);
                }

                // Wait for input or auto-advance
                let nav = if let Some(ms) = cmd.wait.or(config.auto_advance) {
                    thread::sleep(Duration::from_millis(ms));
                    println!();
                    NavAction::Continue
                } else {
                    let action = wait_for_input(has_chapters);
                    // Handle navigation
                    match &action {
                        NavAction::NextChapter => {
                            if let Some(next) = demo.chapters.iter().find(|c| c.start > idx) {
                                idx = next.start;
                                // Recalculate chapter_idx
                                chapter_idx = demo
                                    .chapters
                                    .iter()
                                    .position(|c| c.start == idx)
                                    .unwrap_or(chapter_idx);
                                continue;
                            }
                        }
                        NavAction::PrevChapter => {
                            // Find the chapter that contains the current step
                            let current_chapter =
                                demo.chapters.iter().rposition(|c| c.start <= idx);
                            if let Some(ci) = current_chapter {
                                if ci > 0 {
                                    idx = demo.chapters[ci - 1].start;
                                    chapter_idx = ci - 1;
                                    continue;
                                } else {
                                    idx = demo.chapters[0].start;
                                    chapter_idx = 0;
                                    continue;
                                }
                            }
                        }
                        NavAction::JumpChapter(target) => {
                            if *target < demo.chapters.len() {
                                idx = demo.chapters[*target].start;
                                chapter_idx = *target;
                                continue;
                            }
                        }
                        NavAction::Continue => {
                            println!();
                        }
                    }
                    action
                };
                let _ = nav;

                // Execute the command
                if let Some(ref fake) = cmd.fake_output {
                    // Fake output mode
                    if let Some(output_speed) = cmd.output_speed {
                        let output_delay = speed_to_delay(output_speed);
                        type_text(fake, output_delay, jitter_val, 0);
                        println!();
                    } else {
                        print!("{}", fake);
                        io::stdout().flush().unwrap();
                    }

                    if cmd.execute {
                        // Also run the real command (output is hidden since fake is shown)
                        let before_env = runtime_env.clone();
                        let step_env = merge_env(&runtime_env, &cmd.env);
                        if let Ok((_status, snapshot)) = shell_status_with_env_snapshot(
                            &resolved_text,
                            &step_env,
                            Stdio::null(),
                            Stdio::null(),
                            Stdio::null(),
                        ) {
                            persist_env_snapshot(&mut runtime_env, &before_env, &cmd.env, snapshot);
                        }
                    }
                } else if !cmd.execute {
                    // No execution, no fake output — just typed the command
                } else if let Some(ref pattern) = cmd.wait_for {
                    run_command_wait_for(
                        &resolved_text,
                        pattern,
                        cmd.timeout,
                        &mut runtime_env,
                        &cmd.env,
                    );
                } else if let Some(ref interactions) = cmd.interact {
                    let interaction_refs: Vec<Interaction> = interactions
                        .iter()
                        .map(|i| Interaction {
                            expect: i.expect.clone(),
                            send: i.send.clone(),
                        })
                        .collect();
                    run_command_interact(
                        &resolved_text,
                        &interaction_refs,
                        &mut runtime_env,
                        &cmd.env,
                    );
                } else {
                    let capture_ref = cmd.capture.as_ref().map(|c| Capture {
                        name: c.name.clone(),
                        pattern: c.pattern.clone(),
                        json_path: c.json_path.clone(),
                    });
                    let (captured, _code) = run_command(
                        &resolved_text,
                        capture_ref.as_ref(),
                        &mut runtime_env,
                        &cmd.env,
                        config.show_exit_status,
                    );
                    if let Some(value) = captured {
                        if let Some(ref cap) = cmd.capture {
                            // Available to demonator `{name}` substitution...
                            vars.insert(cap.name.clone(), value.clone());
                            // ...and exported so later steps' shells expand `$name`.
                            runtime_env.insert(cap.name.clone(), value);
                        }
                    }
                }

                if cmd.wait_after {
                    print!("{}", prompt);
                    io::stdout().flush().unwrap();
                    wait_for_any_key_silent();
                    print!("\r\x1B[2K");
                    io::stdout().flush().unwrap();
                }
            }
        }

        idx += 1;
    }

    // Teardown
    if let Some(ref teardown) = config.teardown {
        run_hidden_commands(teardown, &mut runtime_env);
    }
}

// ---------------------------------------------------------------------------
// File watching
// ---------------------------------------------------------------------------

fn file_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn wait_for_file_change(path: &Path) {
    let initial = file_mtime(path);
    loop {
        thread::sleep(Duration::from_millis(500));
        let current = file_mtime(path);
        if current != initial {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_colors_basic() {
        let result = expand_colors("{red}hello{reset}");
        assert_eq!(result, "\x1B[31mhello\x1B[0m");
    }

    #[test]
    fn test_expand_colors_all_colors() {
        assert_eq!(expand_colors("{black}"), "\x1B[30m");
        assert_eq!(expand_colors("{red}"), "\x1B[31m");
        assert_eq!(expand_colors("{green}"), "\x1B[32m");
        assert_eq!(expand_colors("{yellow}"), "\x1B[33m");
        assert_eq!(expand_colors("{blue}"), "\x1B[34m");
        assert_eq!(expand_colors("{magenta}"), "\x1B[35m");
        assert_eq!(expand_colors("{cyan}"), "\x1B[36m");
        assert_eq!(expand_colors("{white}"), "\x1B[37m");
        assert_eq!(expand_colors("{bold}"), "\x1B[1m");
        assert_eq!(expand_colors("{dim}"), "\x1B[2m");
        assert_eq!(expand_colors("{reset}"), "\x1B[0m");
    }

    #[test]
    fn test_expand_colors_no_placeholders() {
        assert_eq!(expand_colors("plain text"), "plain text");
    }

    #[test]
    fn test_expand_colors_multiple() {
        let result = expand_colors("{green}~{reset} {blue}${reset}");
        assert_eq!(result, "\x1B[32m~\x1B[0m \x1B[34m$\x1B[0m");
    }

    #[test]
    fn test_default_values() {
        assert_eq!(default_delay(), 50);
        assert_eq!(default_jitter(), 0);
        assert_eq!(default_pause(), 200);
        assert_eq!(default_prompt(), "{green}~{reset} {blue}${reset} ");
        assert_eq!(speed_to_delay(20), 50);
    }

    #[test]
    fn test_config_deserialize_minimal() {
        let yaml = "steps:\n  - text: 'echo hello'\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.steps.len(), 1);
        match &config.steps[0] {
            Step::Command(cmd) => assert_eq!(cmd.text, "echo hello"),
            _ => panic!("expected Command step"),
        }
        assert_eq!(config.speed, None);
        assert_eq!(config.delay, 50);
        assert_eq!(config.jitter, 0);
        assert_eq!(config.pause, 200);
        assert!(!config.clear);
        assert!(!config.highlight);
        assert_eq!(config.prompt, "{green}~{reset} {blue}${reset} ");
    }

    #[test]
    fn test_config_deserialize_full() {
        let yaml = r#"
speed: 12
delay: 100
jitter: 20
pause: 500
clear: true
prompt: "$ "
steps:
  - text: "echo hello"
  - text: "ls -la"
    speed: 25
    delay: 30
    jitter: 10
    pause: 100
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.speed, Some(12));
        assert_eq!(config.delay, 100);
        assert_eq!(config.jitter, 20);
        assert_eq!(config.pause, 500);
        assert!(config.clear);
        assert_eq!(config.prompt, "$ ");
        assert_eq!(config.steps.len(), 2);
        match &config.steps[1] {
            Step::Command(cmd) => {
                assert_eq!(cmd.speed, Some(25));
                assert_eq!(cmd.delay, Some(30));
                assert_eq!(cmd.jitter, Some(10));
                assert_eq!(cmd.pause, Some(100));
            }
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_step_defaults_to_none() {
        let yaml = "text: 'echo hello'\n";
        let step: Step = serde_yaml::from_str(yaml).unwrap();
        match step {
            Step::Command(cmd) => {
                assert_eq!(cmd.text, "echo hello");
                assert!(cmd.speed.is_none());
                assert!(cmd.delay.is_none());
                assert!(cmd.jitter.is_none());
                assert!(cmd.pause.is_none());
                assert!(cmd.execute);
                assert!(cmd.fake_output.is_none());
                assert!(cmd.wait_for.is_none());
                assert!(cmd.interact.is_none());
                assert!(cmd.if_condition.is_none());
                assert!(cmd.unless.is_none());
            }
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_step_override_resolves_correctly() {
        let yaml = r#"
delay: 80
jitter: 30
pause: 300
steps:
  - text: "cmd1"
  - text: "cmd2"
    delay: 10
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match (&config.steps[0], &config.steps[1]) {
            (Step::Command(s0), Step::Command(s1)) => {
                assert_eq!(resolve_delay(s0, &config), 80);
                assert_eq!(s0.jitter.unwrap_or(config.jitter), 30);
                assert_eq!(s0.pause.unwrap_or(config.pause), 300);

                assert_eq!(resolve_delay(s1, &config), 10);
                assert_eq!(s1.jitter.unwrap_or(config.jitter), 30);
            }
            _ => panic!("expected Command steps"),
        }
    }

    #[test]
    fn test_speed_override_resolves_correctly() {
        let yaml = r#"
speed: 20
delay: 80
jitter: 30
pause: 300
steps:
  - text: "cmd1"
  - text: "cmd2"
    speed: 40
  - text: "cmd3"
    delay: 10
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match (&config.steps[0], &config.steps[1], &config.steps[2]) {
            (Step::Command(s0), Step::Command(s1), Step::Command(s2)) => {
                assert_eq!(resolve_delay(s0, &config), 50);
                assert_eq!(resolve_delay(s1, &config), 25);
                assert_eq!(resolve_delay(s2, &config), 50);
            }
            _ => panic!("expected Command steps"),
        }
    }

    #[test]
    fn test_config_empty_steps() {
        let yaml = "steps: []\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.steps.is_empty());
    }

    #[test]
    fn test_config_no_steps_or_chapters_parses() {
        let yaml = "delay: 50\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.steps.is_empty());
        assert!(config.chapters.is_empty());
    }

    #[test]
    fn test_substitute_vars_basic() {
        let mut vars = HashMap::new();
        vars.insert("session_id".to_string(), "abc123".to_string());
        assert_eq!(
            substitute_vars("nono attach {session_id}", &vars),
            "nono attach abc123"
        );
    }

    #[test]
    fn test_substitute_vars_multiple() {
        let mut vars = HashMap::new();
        vars.insert("host".to_string(), "localhost".to_string());
        vars.insert("port".to_string(), "8080".to_string());
        assert_eq!(
            substitute_vars("curl {host}:{port}", &vars),
            "curl localhost:8080"
        );
    }

    #[test]
    fn test_substitute_vars_no_match() {
        let vars = HashMap::new();
        assert_eq!(substitute_vars("no vars here", &vars), "no vars here");
    }

    #[test]
    fn test_substitute_vars_does_not_replace_colors() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "world".to_string());
        assert_eq!(
            substitute_vars("{red}hello {name}{reset}", &vars),
            "{red}hello world{reset}"
        );
    }

    #[test]
    fn test_capture_deserialize() {
        let yaml = r#"
steps:
  - text: "echo hello"
    capture:
      name: session_id
      pattern: "session (\\w+)"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Command(cmd) => {
                let cap = cmd.capture.as_ref().unwrap();
                assert_eq!(cap.name, "session_id");
                assert_eq!(cap.pattern, Some("session (\\w+)".to_string()));
            }
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_capture_regex_extraction() {
        let pattern = r"Started detached session (\w+)";
        let text = "Started detached session e96400cf26349136.\nAttach with: nono attach e96400cf26349136\n";
        let re = Regex::new(pattern).unwrap();
        let caps = re.captures(text).unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "e96400cf26349136");
    }

    #[test]
    fn test_pause_directive() {
        let yaml = r#"
steps:
  - pause
  - text: "echo hello"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.steps.len(), 2);
        match &config.steps[0] {
            Step::Directive(d) => assert_eq!(d, "pause"),
            _ => panic!("expected Directive step"),
        }
        match &config.steps[1] {
            Step::Command(cmd) => assert_eq!(cmd.text, "echo hello"),
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_timed_pause_step() {
        let yaml = r#"
steps:
  - pause: 20
  - comment: "Narration"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.steps.len(), 2);
        match &config.steps[0] {
            Step::TimedPause(p) => assert_eq!(p.pause, 20),
            _ => panic!("expected TimedPause step"),
        }
        match &config.steps[1] {
            Step::Comment(c) => assert_eq!(c.comment, "Narration"),
            _ => panic!("expected Comment step"),
        }
    }

    #[test]
    fn test_clear_directive() {
        let yaml = r#"
steps:
  - text: "echo hello"
  - clear
  - text: "echo world"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.steps.len(), 3);
        match &config.steps[1] {
            Step::Directive(d) => assert_eq!(d, "clear"),
            _ => panic!("expected Directive step"),
        }
    }

    #[test]
    fn test_step_without_capture() {
        let yaml = r#"
steps:
  - text: "echo hello"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Command(cmd) => assert!(cmd.capture.is_none()),
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_json_path_capture_deserialize() {
        let yaml = r#"
steps:
  - text: "nono audit list --json"
    capture:
      name: session_id
      json_path: "[0].session_id"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Command(cmd) => {
                let cap = cmd.capture.as_ref().unwrap();
                assert_eq!(cap.name, "session_id");
                assert!(cap.pattern.is_none());
                assert_eq!(cap.json_path, Some("[0].session_id".to_string()));
            }
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_extract_json_path_array_index() {
        let json = r#"[{"session_id": "abc123"}, {"session_id": "def456"}]"#;
        assert_eq!(
            extract_json_path(json, "[0].session_id"),
            Some("abc123".to_string())
        );
        assert_eq!(
            extract_json_path(json, "[1].session_id"),
            Some("def456".to_string())
        );
    }

    #[test]
    fn test_extract_json_path_nested() {
        let json = r#"{"data": {"items": [{"id": 42}]}}"#;
        assert_eq!(
            extract_json_path(json, "data.items[0].id"),
            Some("42".to_string())
        );
    }

    #[test]
    fn test_extract_json_path_simple_key() {
        let json = r#"{"name": "hello"}"#;
        assert_eq!(extract_json_path(json, "name"), Some("hello".to_string()));
    }

    #[test]
    fn test_extract_json_path_no_match() {
        let json = r#"{"name": "hello"}"#;
        assert_eq!(extract_json_path(json, "missing"), None);
    }

    #[test]
    fn test_extract_json_path_invalid_json() {
        assert_eq!(extract_json_path("not json", "foo"), None);
    }

    // --- New feature tests ---

    #[test]
    fn test_comment_step() {
        let yaml = r#"
steps:
  - comment: "This is a narration"
    style: dim
    speed: 25
    delay: 30
    jitter: 15
    pause: 100
  - text: "echo hello"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.steps.len(), 2);
        match &config.steps[0] {
            Step::Comment(c) => {
                assert_eq!(c.comment, "This is a narration");
                assert_eq!(c.style.as_deref(), Some("dim"));
                assert_eq!(c.speed, Some(25));
                assert_eq!(c.delay, Some(30));
                assert_eq!(c.jitter, Some(15));
                assert_eq!(c.pause, Some(100));
            }
            _ => panic!("expected Comment step"),
        }
    }

    #[test]
    fn test_comment_step_no_style() {
        let yaml = r#"
steps:
  - comment: "Just a note"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Comment(c) => {
                assert_eq!(c.comment, "Just a note");
                assert!(c.style.is_none());
                assert!(c.speed.is_none());
                assert!(c.delay.is_none());
                assert!(c.jitter.is_none());
                assert!(c.pause.is_none());
            }
            _ => panic!("expected Comment step"),
        }
    }

    #[test]
    fn test_comment_timing_resolves_correctly() {
        let yaml = r#"
speed: 20
delay: 80
jitter: 30
pause: 300
steps:
  - comment: "global timing"
  - comment: "custom timing"
    speed: 40
    delay: 10
    jitter: 5
    pause: 50
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match (&config.steps[0], &config.steps[1]) {
            (Step::Comment(c0), Step::Comment(c1)) => {
                assert_eq!(resolve_comment_delay(c0, &config), 50);
                assert_eq!(c0.jitter.unwrap_or(config.jitter), 30);
                assert_eq!(c0.pause.unwrap_or(config.pause), 300);

                assert_eq!(resolve_comment_delay(c1, &config), 25);
                assert_eq!(c1.jitter.unwrap_or(config.jitter), 5);
                assert_eq!(c1.pause.unwrap_or(config.pause), 50);
            }
            _ => panic!("expected Comment steps"),
        }
    }

    #[test]
    fn test_fake_output() {
        let yaml = r#"
steps:
  - text: "curl https://api.example.com/health"
    fake_output: '{"status": "ok"}'
    execute: false
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Command(cmd) => {
                assert_eq!(cmd.fake_output.as_deref(), Some("{\"status\": \"ok\"}"));
                assert!(!cmd.execute);
            }
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_auto_advance() {
        let yaml = r#"
auto_advance: 1500
steps:
  - text: "echo hello"
    wait: 2000
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.auto_advance, Some(1500));
        match &config.steps[0] {
            Step::Command(cmd) => assert_eq!(cmd.wait, Some(2000)),
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_wait_for_pattern() {
        let yaml = r#"
steps:
  - text: "docker-compose logs -f"
    wait_for: "Listening on port 8080"
    timeout: 60
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Command(cmd) => {
                assert_eq!(cmd.wait_for.as_deref(), Some("Listening on port 8080"));
                assert_eq!(cmd.timeout, 60);
            }
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_setup_teardown() {
        let yaml = r#"
setup:
  - "mkdir -p /tmp/test"
  - "echo setup"
teardown:
  - "rm -rf /tmp/test"
steps:
  - text: "ls /tmp/test"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.setup.as_ref().unwrap().len(), 2);
        assert_eq!(config.teardown.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_exported_env_persists_between_commands() {
        let mut env_state = HashMap::new();
        let overlay = HashMap::new();

        let (_captured, code) = run_command(
            "export DEMONATOR_TEST_TOKEN=s3cr3t_t0k3n",
            None,
            &mut env_state,
            &overlay,
            true,
        );
        assert_eq!(code, 0);
        assert_eq!(
            env_state.get("DEMONATOR_TEST_TOKEN").map(String::as_str),
            Some("s3cr3t_t0k3n")
        );

        let (captured, code) = run_command(
            "printf '%s' \"$DEMONATOR_TEST_TOKEN\"",
            Some(&Capture {
                name: "token".to_string(),
                pattern: Some("(.*)".to_string()),
                json_path: None,
            }),
            &mut env_state,
            &overlay,
            true,
        );
        assert_eq!(code, 0);
        assert_eq!(captured.as_deref(), Some("s3cr3t_t0k3n"));
    }

    #[test]
    fn test_chapters() {
        let yaml = r#"
chapters:
  - name: "Setup"
    steps:
      - text: "git clone repo"
  - name: "Build"
    steps:
      - text: "cargo build"
      - text: "cargo test"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.chapters.len(), 2);
        assert_eq!(config.chapters[0].name, "Setup");
        assert_eq!(config.chapters[0].steps.len(), 1);
        assert_eq!(config.chapters[1].name, "Build");
        assert_eq!(config.chapters[1].steps.len(), 2);
    }

    #[test]
    fn test_resolve_demo_chapters() {
        let yaml = r#"
chapters:
  - name: "A"
    steps:
      - text: "cmd1"
      - text: "cmd2"
  - name: "B"
    steps:
      - text: "cmd3"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let demo = resolve_demo(&config);
        assert_eq!(demo.steps.len(), 3);
        assert_eq!(demo.chapters.len(), 2);
        assert_eq!(demo.chapters[0].name, "A");
        assert_eq!(demo.chapters[0].start, 0);
        assert_eq!(demo.chapters[1].name, "B");
        assert_eq!(demo.chapters[1].start, 2);
    }

    #[test]
    fn test_interact_deserialize() {
        let yaml = r#"
steps:
  - text: "npm init"
    interact:
      - expect: "package name:"
        send: "my-app"
      - expect: "version:"
        send: "1.0.0"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Command(cmd) => {
                let interactions = cmd.interact.as_ref().unwrap();
                assert_eq!(interactions.len(), 2);
                assert_eq!(interactions[0].expect.as_deref(), Some("package name:"));
                assert_eq!(interactions[0].send, "my-app");
                assert_eq!(interactions[1].expect.as_deref(), Some("version:"));
                assert_eq!(interactions[1].send, "1.0.0");
            }
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_interact_no_expect() {
        let yaml = r#"
steps:
  - text: "my-cmd"
    interact:
      - send: "my-cool-app"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Command(cmd) => {
                let interactions = cmd.interact.as_ref().unwrap();
                assert_eq!(interactions.len(), 1);
                assert_eq!(interactions[0].expect, None);
                assert_eq!(interactions[0].send, "my-cool-app");
            }
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_conditional_if() {
        let yaml = r#"
steps:
  - text: "docker build ."
    if: has_docker
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Command(cmd) => {
                assert_eq!(cmd.if_condition.as_deref(), Some("has_docker"));
            }
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_conditional_unless() {
        let yaml = r#"
steps:
  - text: "podman build ."
    unless: has_docker
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Command(cmd) => {
                assert_eq!(cmd.unless.as_deref(), Some("has_docker"));
            }
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_should_run_step_if_present() {
        let cmd = CommandStep {
            text: "test".to_string(),
            speed: None,
            delay: None,
            jitter: None,
            pause: None,
            capture: None,
            fake_output: None,
            output_speed: None,
            execute: true,
            wait_for: None,
            timeout: 30,
            wait: None,
            interact: None,
            if_condition: Some("myvar".to_string()),
            unless: None,
            wait_before: false,
            wait_after: false,
            env: HashMap::new(),
            hidden: false,
        };

        let mut vars = HashMap::new();
        assert!(!should_run_step(&cmd, &vars));

        vars.insert("myvar".to_string(), "value".to_string());
        assert!(should_run_step(&cmd, &vars));

        vars.insert("myvar".to_string(), "  ".to_string());
        assert!(!should_run_step(&cmd, &vars));
    }

    #[test]
    fn test_should_run_step_unless_present() {
        let cmd = CommandStep {
            text: "test".to_string(),
            speed: None,
            delay: None,
            jitter: None,
            pause: None,
            capture: None,
            fake_output: None,
            output_speed: None,
            execute: true,
            wait_for: None,
            timeout: 30,
            wait: None,
            interact: None,
            if_condition: None,
            unless: Some("myvar".to_string()),
            wait_before: false,
            wait_after: false,
            env: HashMap::new(),
            hidden: false,
        };

        let vars = HashMap::new();
        assert!(should_run_step(&cmd, &vars));

        let mut vars = HashMap::new();
        vars.insert("myvar".to_string(), "value".to_string());
        assert!(!should_run_step(&cmd, &vars));
    }

    #[test]
    fn test_highlight_command_basic() {
        let tokens = highlight_command("echo hello");
        assert!(!tokens.is_empty());
        // First word should be bold white
        assert_eq!(tokens[0], ('e', HL_BOLD_WHITE));
        assert_eq!(tokens[1], ('c', HL_BOLD_WHITE));
        assert_eq!(tokens[2], ('h', HL_BOLD_WHITE));
        assert_eq!(tokens[3], ('o', HL_BOLD_WHITE));
    }

    #[test]
    fn test_highlight_command_flags() {
        let tokens = highlight_command("ls --all -l");
        // Find the -- flags
        let flag_chars: Vec<_> = tokens
            .iter()
            .filter(|(_, color)| *color == HL_YELLOW)
            .collect();
        assert!(flag_chars.len() >= 6); // --all + -l
    }

    #[test]
    fn test_highlight_command_strings() {
        let tokens = highlight_command("echo \"hello world\"");
        let string_chars: Vec<_> = tokens
            .iter()
            .filter(|(_, color)| *color == HL_GREEN)
            .collect();
        assert!(string_chars.len() >= 13); // "hello world"
    }

    #[test]
    fn test_highlight_command_pipe() {
        let tokens = highlight_command("cat file | grep pattern");
        let pipe_chars: Vec<_> = tokens
            .iter()
            .filter(|(ch, color)| *ch == '|' && *color == HL_CYAN)
            .collect();
        assert_eq!(pipe_chars.len(), 1);
    }

    #[test]
    fn test_highlight_command_variable() {
        let tokens = highlight_command("echo $HOME");
        let var_chars: Vec<_> = tokens
            .iter()
            .filter(|(_, color)| *color == HL_MAGENTA)
            .collect();
        assert!(var_chars.len() >= 5); // $HOME
    }

    #[test]
    fn test_highlight_config() {
        let yaml = r#"
highlight: true
steps:
  - text: "echo hello"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.highlight);
    }

    #[test]
    fn test_style_to_ansi() {
        assert_eq!(style_to_ansi(Some("dim")), "\x1B[2m");
        assert_eq!(style_to_ansi(Some("bold")), "\x1B[1m");
        assert_eq!(style_to_ansi(Some("italic")), "\x1B[3m");
        assert_eq!(style_to_ansi(Some("red")), "\x1B[31m");
        assert_eq!(style_to_ansi(None), "\x1B[2m");
    }

    #[test]
    fn test_output_speed() {
        let yaml = r#"
steps:
  - text: "curl example.com"
    fake_output: "response body"
    output_speed: 40
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Command(cmd) => {
                assert_eq!(cmd.output_speed, Some(40));
            }
            _ => panic!("expected Command step"),
        }
    }

    #[test]
    fn test_hidden_step() {
        let yaml = r#"
steps:
  - text: "nono audit list --recent 1 --json"
    hidden: true
    capture:
      name: session_id
      json_path: "[0].session_id"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match &config.steps[0] {
            Step::Command(cmd) => {
                assert!(cmd.hidden);
                assert_eq!(cmd.capture.as_ref().unwrap().name, "session_id");
            }
            _ => panic!("expected Command step"),
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    if cli.watch {
        loop {
            let config = load_config(&cli.config);
            run_demo(&config, &cli);
            eprintln!(
                "\n\x1B[2m[demonator] watching {} for changes... (Ctrl+C to exit)\x1B[0m",
                cli.config.display()
            );
            wait_for_file_change(&cli.config);
            // Clear screen before re-running
            print!("\x1B[2J\x1B[H");
            io::stdout().flush().unwrap();
        }
    } else {
        let config = load_config(&cli.config);
        run_demo(&config, &cli);
    }
}
