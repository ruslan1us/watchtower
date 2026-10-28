//! Slash-command suggestions in the task field (doc/0042).
//!
//! **What is offered.** The skills and custom commands Claude Code shows in
//! its own `/` menu, from three sources (rules in
//! `doc/reference/claude-code-cli.md`, "Skills and custom commands"):
//!
//! - *personal*: `<config dir>/skills/<name>/SKILL.md` and
//!   `<config dir>/commands/**/*.md` of the account (its
//!   `CLAUDE_CONFIG_DIR`, else `~/.claude`);
//! - *project*: `.claude/skills` and `.claude/commands` of the folder and of
//!   every parent up to the repository root;
//! - *plugin*: the enabled plugins `<account command> plugin list --json`
//!   reports, read under the folder it names (`readFromFolder`, else
//!   `installPath`), namespaced `/<plugin>:<name>`.
//!
//! Built-in commands, bundled skills, managed (enterprise) skills, MCP
//! prompts, workflows and skills synced from claude.ai are not offered.
//!
//! **What is read** ([`read_head`]): only the start of each file, up to the
//! closing `---` of its frontmatter (and, without a `description`, up to the
//! first non-empty line after it), at most [`MAX_HEAD_BYTES`]; a plugin's
//! `.claude-plugin/plugin.json`, at most [`MAX_MANIFEST_BYTES`]. Folder
//! listings stop after [`MAX_DIR_ENTRIES`] entries, the whole list after
//! [`MAX_COMMANDS`]. Hidden entries and files that cannot be read, are not
//! UTF-8 or whose frontmatter does not close are skipped silently. Nothing
//! is ever written. The load runs on a worker thread ([`CommandLoad`]).
//!
//! **The menu** ([`SlashMenu`], no I/O): opens while `/` is the first
//! non-blank character of the text and the cursor is inside that first
//! word; it lists the matching entries (prefix matches first, then
//! substring), `↑` / `↓` choose, `Tab` / `Enter` accept, `Esc` closes only
//! the menu. The window that hosts it (the new-session dialog's task field,
//! the quick reply window) gives it its keys first ([`SlashMenu::handle_key`])
//! and calls [`SlashMenu::sync`] after every key.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::cli::CliRunner;
use crate::config::Account;
use crate::dialog::TextArea;

/// At most this much of a skill or command file is read (16 KiB).
pub const MAX_HEAD_BYTES: usize = 16 * 1024;
/// A plugin manifest larger than this (64 KiB) is ignored.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// A folder listing stops after this many entries.
pub const MAX_DIR_ENTRIES: usize = 512;
/// The whole list stops at this many entries.
pub const MAX_COMMANDS: usize = 2000;
/// At most this many plugins are read.
pub const MAX_PLUGINS: usize = 64;
/// Command files are looked for at most this many folders below
/// `commands/` (`commands/a/b/c/x.md` is `/a:b:c:x`).
pub const MAX_COMMAND_DEPTH: usize = 3;
/// At most this many parent folders are looked at for the repository root.
pub const MAX_PARENT_LEVELS: usize = 16;
/// The timeout of `plugin list --json`.
pub const PLUGIN_LIST_TIMEOUT: Duration = Duration::from_secs(10);
/// The arguments of the plugin listing call.
pub const PLUGIN_LIST_ARGS: [&str; 3] = ["plugin", "list", "--json"];
/// A description is kept to at most this many characters.
const MAX_DESCRIPTION_CHARS: usize = 300;

// ---- the list ----------------------------------------------------------------

/// Where an entry comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Origin {
    /// The account's own `skills` / `commands` folders.
    Personal,
    /// The project's `.claude/skills` / `.claude/commands`.
    Project,
    /// An enabled plugin, by its namespace (the manifest's `name`).
    Plugin(String),
}

impl Origin {
    /// The menu's source tag: `personal`, `project`, `plugin <name>`.
    pub fn tag(&self) -> String {
        match self {
            Origin::Personal => "personal".to_owned(),
            Origin::Project => "project".to_owned(),
            Origin::Plugin(name) => format!("plugin {name}"),
        }
    }
}

/// A skill (`<name>/SKILL.md`) or a command file (`*.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Skill,
    Command,
}

/// One entry of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    /// What follows the `/`, e.g. `fix-bug` or `review-kit:code-review`.
    pub name: String,
    /// The frontmatter `description`, else the first non-empty line of the
    /// text; one line, may be empty.
    pub description: String,
    /// The frontmatter `argument-hint`, one line.
    pub argument_hint: Option<String>,
    /// Where it comes from.
    pub origin: Origin,
    /// A skill or a command file.
    pub kind: Kind,
}

/// What a load found, sorted by name ignoring case, one entry per name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandList {
    pub commands: Vec<SlashCommand>,
}

/// The account and folder a list is for: the cache key (doc/0042).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandKey {
    /// The account's name.
    pub account: String,
    /// The project folder (the dialog's project, the reply's `cwd`).
    pub folder: PathBuf,
}

/// A load of the commands for `account` in `folder`, run on a worker
/// thread; the result goes to `App::commands_loaded`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLoad {
    /// Which load this is; the newest per key wins.
    pub id: u64,
    /// The account: its config dir holds the personal entries and its
    /// command lists the plugins (doc/0003).
    pub account: Account,
    /// The project folder.
    pub folder: PathBuf,
}

impl CommandLoad {
    /// The cache key of this load.
    pub fn key(&self) -> CommandKey {
        CommandKey {
            account: self.account.name.clone(),
            folder: self.folder.clone(),
        }
    }

    /// Runs the load: the personal and project folders, then
    /// `<account command> plugin list --json` in the folder (with
    /// [`PLUGIN_LIST_TIMEOUT`]) and the enabled plugins' folders. A failed
    /// plugin listing only leaves the plugins out.
    pub fn run(&self, runner: &CliRunner) -> CommandList {
        let personal = personal(&self.account.config_dir);
        let project = project(&self.folder, &self.account.config_dir);
        let runner = runner.clone().with_timeout(PLUGIN_LIST_TIMEOUT);
        let installs = match runner.run_in(&self.account, &self.folder, PLUGIN_LIST_ARGS) {
            Ok(out) if out.success() => parse_plugin_list(&out.stdout_text(), &self.folder),
            _ => Vec::new(),
        };
        let mut plugins = Vec::new();
        for install in &installs {
            plugins.extend(plugin(&install.root, &install.name));
            if plugins.len() >= MAX_COMMANDS {
                break;
            }
        }
        merge(personal, project, plugins)
    }
}

/// The final list: personal and project entries by precedence (a skill
/// over a command file, then personal over project; within one source the
/// first found), then the plugins' (namespaced, the first plugin with a
/// name wins), sorted by name ignoring case, at most [`MAX_COMMANDS`].
pub fn merge(
    personal: Vec<SlashCommand>,
    project: Vec<SlashCommand>,
    plugins: Vec<SlashCommand>,
) -> CommandList {
    let mut plain: Vec<SlashCommand> = personal.into_iter().chain(project).collect();
    let rank = |c: &SlashCommand| match (c.kind, &c.origin) {
        (Kind::Skill, Origin::Personal) => 0,
        (Kind::Skill, _) => 1,
        (Kind::Command, Origin::Personal) => 2,
        (Kind::Command, _) => 3,
    };
    // Stable: the first found keeps its place among equals.
    plain.sort_by_key(rank);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in plain.into_iter().chain(plugins) {
        if seen.insert(c.name.clone()) {
            out.push(c);
        }
    }
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    out.truncate(MAX_COMMANDS);
    CommandList { commands: out }
}

// ---- names -------------------------------------------------------------------

/// A zero-width or bidirectional-formatting character, which Claude Code
/// refuses in a name.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// One segment of a command name (a skill, a file stem, a folder, a
/// frontmatter `name`): not empty, not hidden, no whitespace, control,
/// zero-width or bidirectional characters, no `<`, `>`, `/`, `\` or `:`.
pub fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.chars().count() <= 128
        && !s.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || invisible(c)
                || matches!(c, '<' | '>' | '/' | '\\' | ':')
        })
}

/// A reserved skill folder or name (`synced`, `anthropic-skills`).
fn reserved(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower == "synced" || lower == "anthropic-skills"
}

/// A plugin namespace: as a segment, without `@` either (doc rule for a
/// manifest `name`).
fn valid_plugin_name(s: &str) -> bool {
    valid_segment(s) && !s.contains('@')
}

// ---- reading a file's head -----------------------------------------------------

/// The parts of a skill or command file the menu uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Head {
    /// The frontmatter, when the file starts with `---`.
    pub frontmatter: Option<Frontmatter>,
    /// The first non-empty line after the frontmatter (of the whole text
    /// without one), trimmed; looked for only while no `description` was
    /// found.
    pub first_line: Option<String>,
}

/// The frontmatter fields the menu uses. A frontmatter that is not valid
/// YAML gives an empty one with `malformed` set: Claude Code then loads
/// the file with empty metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    /// `user-invocable`: `false` hides the entry from the `/` menu.
    pub user_invocable: Option<bool>,
    /// The YAML did not parse; the fields above are all empty.
    pub malformed: bool,
}

/// Splits `text` (the start of a file) into its frontmatter and first
/// line. `None` when the frontmatter does not close: a file cut at the
/// read cap, or one Claude Code cannot read either.
pub fn parse_head(text: &str) -> Option<Head> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut lines = text.lines();
    let mut head = Head::default();
    let mut body: Vec<&str> = Vec::new();
    let mut first = lines.next();
    if first.is_some_and(|l| l.trim_end() == "---") {
        first = None;
        let mut fm = Vec::new();
        let mut closed = false;
        for line in lines.by_ref() {
            if line.trim_end() == "---" {
                closed = true;
                break;
            }
            fm.push(line);
        }
        if !closed {
            return None;
        }
        head.frontmatter = Some(parse_frontmatter(&fm));
    }
    if head
        .frontmatter
        .as_ref()
        .is_some_and(|f| f.description.is_some())
    {
        return Some(head);
    }
    body.extend(first);
    body.extend(lines);
    head.first_line = body
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(str::to_owned);
    Some(head)
}

/// Reads the head of `path`: a regular file (after following links), opened
/// without blocking on a FIFO, read line by line up to the end of what
/// [`parse_head`] needs and at most [`MAX_HEAD_BYTES`]. `None` when it
/// cannot be read, is not UTF-8 or its frontmatter does not close.
pub fn read_head(path: &Path) -> Option<Head> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut reader = BufReader::new(file.take(MAX_HEAD_BYTES as u64));
    let mut bytes: Vec<u8> = Vec::new();
    let mut line_no = 0usize;
    // 0: first line; 1: in the frontmatter; 2: after it (or none), looking
    // for the first non-empty line.
    let mut stage = 0u8;
    let mut fm_start = 0usize;
    loop {
        let start = bytes.len();
        let n = reader.read_until(b'\n', &mut bytes).ok()?;
        if n == 0 {
            break;
        }
        let raw = &bytes[start..];
        let line = std::str::from_utf8(raw).unwrap_or("");
        let line = if line_no == 0 {
            line.strip_prefix('\u{FEFF}').unwrap_or(line)
        } else {
            line
        };
        let trimmed = line.trim_end_matches(['\n', '\r']).trim_end();
        match stage {
            0 if trimmed == "---" => {
                stage = 1;
                fm_start = bytes.len();
            }
            0 => {
                stage = 2;
                if !trimmed.trim().is_empty() {
                    break;
                }
            }
            1 if trimmed == "---" => {
                let fm_text = String::from_utf8_lossy(&bytes[fm_start..start]).into_owned();
                let lines: Vec<&str> = fm_text.lines().collect();
                if parse_frontmatter(&lines).description.is_some() {
                    break;
                }
                stage = 2;
            }
            1 => {}
            _ => {
                if !trimmed.trim().is_empty() {
                    break;
                }
            }
        }
        line_no += 1;
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        // Cut inside a character at the cap: keep the whole ones.
        Err(err) if err.utf8_error().error_len().is_none() => {
            let valid = err.utf8_error().valid_up_to();
            let mut bytes = err.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).ok()?
        }
        Err(_) => return None,
    };
    parse_head(&text)
}

// ---- a small YAML reader for the frontmatter -------------------------------------

/// Reads the top-level `key: value` pairs of a frontmatter: plain, single-
/// and double-quoted scalars (also over several lines), `|` and `>` block
/// scalars, and flow collections kept as written (`argument-hint:
/// [issue-number]` shows as `[issue-number]`). Comments and nested values
/// are skipped. A line that is none of these makes it `malformed`.
pub fn parse_frontmatter(lines: &[&str]) -> Frontmatter {
    let mut fm = Frontmatter::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t']) || t.starts_with("- ") || t == "-" {
            // Part of a nested value whose key was skipped.
            continue;
        }
        let Some((key, rest)) = split_key(line) else {
            return Frontmatter {
                malformed: true,
                ..Frontmatter::default()
            };
        };
        // The indented lines that belong to this key.
        let mut more = Vec::new();
        while i < lines.len() {
            let next = lines[i];
            if next.trim().is_empty() || next.starts_with([' ', '\t']) {
                more.push(next);
                i += 1;
            } else {
                break;
            }
        }
        while more.last().is_some_and(|l| l.trim().is_empty()) {
            more.pop();
        }
        if !seen.insert(key.to_owned()) {
            // A duplicate key: YAML parsers refuse it.
            return Frontmatter {
                malformed: true,
                ..Frontmatter::default()
            };
        }
        let Some(value) = scalar(rest, &more) else {
            continue;
        };
        match key {
            "name" => fm.name = value.text(),
            "description" => fm.description = value.text(),
            "argument-hint" => fm.argument_hint = value.text(),
            "user-invocable" => fm.user_invocable = value.boolean(),
            _ => {}
        }
    }
    fm
}

/// `key: rest` of a top-level line; the key is a plain word.
fn split_key(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let key = line[..colon].trim_end();
    let rest = &line[colon + 1..];
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        || !(rest.is_empty() || rest.starts_with([' ', '\t']))
    {
        return None;
    }
    Some((key, rest.trim()))
}

/// A scalar value: its text and whether it was a plain (unquoted) word.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scalar {
    text: String,
    plain: bool,
}

impl Scalar {
    /// The text on one line (whitespace runs collapsed), `None` when empty.
    fn text(&self) -> Option<String> {
        let one: Vec<&str> = self.text.split_whitespace().collect();
        let one = one.join(" ");
        (!one.is_empty()).then_some(one)
    }

    /// A YAML boolean: only unquoted `true` / `false` (any case).
    fn boolean(&self) -> Option<bool> {
        if !self.plain {
            return None;
        }
        match self.text.trim().to_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }
}

/// The value of a key: `rest` on the key's line and the indented lines
/// after it. `None` for a nested mapping or sequence, or a quote that does
/// not close.
fn scalar(rest: &str, more: &[&str]) -> Option<Scalar> {
    let joined = || -> String { more.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ") };
    if rest.is_empty() {
        let first = more.iter().map(|l| l.trim()).find(|l| !l.is_empty())?;
        if first.starts_with("- ") || first == "-" || split_key(first).is_some() {
            return None;
        }
        return Some(Scalar {
            text: strip_comment(&joined()).to_owned(),
            plain: true,
        });
    }
    if rest.starts_with(['|', '>']) {
        let indicator = rest.split_whitespace().next().unwrap_or("");
        if !indicator
            .chars()
            .skip(1)
            .all(|c| matches!(c, '-' | '+') || c.is_ascii_digit())
        {
            return None;
        }
        let folded = rest.starts_with('>');
        let lines: Vec<&str> = more.iter().map(|l| l.trim()).collect();
        let text = if folded {
            lines.join(" ")
        } else {
            lines.join("\n")
        };
        return Some(Scalar { text, plain: false });
    }
    if let Some(body) = rest.strip_prefix('"') {
        let all = std::iter::once(body)
            .chain(more.iter().map(|l| l.trim()))
            .collect::<Vec<_>>()
            .join(" ");
        return double_quoted(&all).map(|text| Scalar { text, plain: false });
    }
    if let Some(body) = rest.strip_prefix('\'') {
        let all = std::iter::once(body)
            .chain(more.iter().map(|l| l.trim()))
            .collect::<Vec<_>>()
            .join(" ");
        return single_quoted(&all).map(|text| Scalar { text, plain: false });
    }
    if rest.starts_with(['[', '{']) {
        // A flow collection, kept as written.
        let mut text = rest.to_owned();
        for l in more {
            text.push(' ');
            text.push_str(l.trim());
        }
        return Some(Scalar {
            text: strip_comment(&text).to_owned(),
            plain: false,
        });
    }
    let mut text = strip_comment(rest).to_owned();
    for l in more {
        let l = strip_comment(l.trim());
        if !l.is_empty() {
            text.push(' ');
            text.push_str(l);
        }
    }
    Some(Scalar { text, plain: true })
}

/// A plain value without its ` #` comment.
fn strip_comment(s: &str) -> &str {
    if s.starts_with('#') {
        return "";
    }
    match s.find(" #").or_else(|| s.find("\t#")) {
        Some(at) => s[..at].trim_end(),
        None => s.trim_end(),
    }
}

/// The text of a double-quoted scalar after its opening quote, with the
/// usual escapes; `None` without a closing quote.
fn double_quoted(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                ' ' => out.push(' '),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            c => out.push(c),
        }
    }
    None
}

/// The text of a single-quoted scalar after its opening quote (`''` is a
/// quote); `None` without a closing quote.
fn single_quoted(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            return Some(out);
        }
        out.push(c);
    }
    None
}

// ---- entries -----------------------------------------------------------------------

/// The entry for a file with `head`: hidden when `user-invocable: false`.
/// `name` is the folder or file name; a valid frontmatter `name` replaces
/// it when `use_name` (skills, not command files). `prefix` namespaces it
/// (`<plugin>:`, command subfolders).
fn entry(
    head: &Head,
    name: &str,
    use_name: bool,
    prefix: Option<&str>,
    origin: &Origin,
    kind: Kind,
) -> Option<SlashCommand> {
    let empty = Frontmatter::default();
    let fm = head.frontmatter.as_ref().unwrap_or(&empty);
    if fm.user_invocable == Some(false) {
        return None;
    }
    let own = fm
        .name
        .as_deref()
        .filter(|n| use_name && valid_segment(n) && !reserved(n));
    let last = own.unwrap_or(name);
    if !valid_segment(last) || reserved(last) {
        return None;
    }
    let name = match prefix {
        Some(p) => format!("{p}:{last}"),
        None => last.to_owned(),
    };
    let description = fm
        .description
        .clone()
        .or_else(|| head.first_line.as_deref().map(display_line))
        .unwrap_or_default();
    Some(SlashCommand {
        name,
        description: cap(&description),
        argument_hint: fm.argument_hint.as_deref().map(cap),
        origin: origin.clone(),
        kind,
    })
}

/// A body line as a description: a Markdown heading's `#` marks dropped,
/// whitespace runs collapsed.
fn display_line(line: &str) -> String {
    let line = line.trim_start_matches('#').trim();
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most [`MAX_DESCRIPTION_CHARS`] characters, on one line.
fn cap(s: &str) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(MAX_DESCRIPTION_CHARS).collect()
}

/// The names in `dir`, sorted, hidden ones left out, at most
/// [`MAX_DIR_ENTRIES`] looked at; non-UTF-8 names are skipped.
fn names_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .take(MAX_DIR_ENTRIES)
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// Whether `path` (followed) stays inside `root` (canonical); always true
/// without a root.
fn contained(path: &Path, root: Option<&Path>) -> bool {
    match root {
        None => true,
        Some(root) => fs::canonicalize(path).is_ok_and(|p| p.starts_with(root)),
    }
}

/// The skills in `dir`: one `<name>/SKILL.md` per folder (links followed;
/// with `root`, only while they stay inside it).
fn scan_skills(
    dir: &Path,
    prefix: Option<&str>,
    origin: &Origin,
    root: Option<&Path>,
    out: &mut Vec<SlashCommand>,
) {
    if !contained(dir, root) {
        return;
    }
    for name in names_in(dir) {
        if out.len() >= MAX_COMMANDS {
            return;
        }
        if reserved(&name) {
            continue;
        }
        let folder = dir.join(&name);
        if !folder.is_dir() || !contained(&folder, root) {
            continue;
        }
        let file = folder.join("SKILL.md");
        if !contained(&file, root) {
            continue;
        }
        if let Some(head) = read_head(&file)
            && let Some(c) = entry(&head, &name, true, prefix, origin, Kind::Skill)
        {
            out.push(c);
        }
    }
}

/// The command files in `dir` and its subfolders, at most
/// [`MAX_COMMAND_DEPTH`] deep: `a/b.md` is `<prefix>:a:b`.
fn scan_commands(
    dir: &Path,
    prefix: Option<&str>,
    origin: &Origin,
    root: Option<&Path>,
    out: &mut Vec<SlashCommand>,
) {
    if contained(dir, root) {
        scan_commands_in(dir, prefix.map(str::to_owned), 0, origin, root, out);
    }
}

fn scan_commands_in(
    dir: &Path,
    prefix: Option<String>,
    depth: usize,
    origin: &Origin,
    root: Option<&Path>,
    out: &mut Vec<SlashCommand>,
) {
    for name in names_in(dir) {
        if out.len() >= MAX_COMMANDS {
            return;
        }
        let path = dir.join(&name);
        if !contained(&path, root) {
            continue;
        }
        if path.is_dir() {
            if depth < MAX_COMMAND_DEPTH && valid_segment(&name) {
                let sub = match &prefix {
                    Some(p) => format!("{p}:{name}"),
                    None => name.clone(),
                };
                scan_commands_in(&path, Some(sub), depth + 1, origin, root, out);
            }
            continue;
        }
        let Some(stem) = name.strip_suffix(".md").filter(|s| !s.is_empty()) else {
            continue;
        };
        if let Some(head) = read_head(&path)
            && let Some(c) = entry(&head, stem, false, prefix.as_deref(), origin, Kind::Command)
        {
            out.push(c);
        }
    }
}

/// The personal entries of the account whose config dir is `config_dir`:
/// `skills/` (without `synced/`, claude.ai's copies) and `commands/`.
pub fn personal(config_dir: &Path) -> Vec<SlashCommand> {
    let mut out = Vec::new();
    scan_skills(
        &config_dir.join("skills"),
        None,
        &Origin::Personal,
        None,
        &mut out,
    );
    scan_commands(
        &config_dir.join("commands"),
        None,
        &Origin::Personal,
        None,
        &mut out,
    );
    out
}

/// The folders whose `.claude` holds project entries for `folder`: the
/// repository root first, then each folder down to `folder` itself. Outside
/// a repository, only `folder`.
pub fn project_folders(folder: &Path) -> Vec<PathBuf> {
    let mut chain: Vec<PathBuf> = Vec::new();
    let mut at = Some(folder);
    let mut root = None;
    while let Some(dir) = at {
        chain.push(dir.to_path_buf());
        if dir.join(".git").exists() {
            root = Some(chain.len());
            break;
        }
        if chain.len() > MAX_PARENT_LEVELS {
            break;
        }
        at = dir.parent();
    }
    let mut dirs: Vec<PathBuf> = match root {
        Some(n) => chain.into_iter().take(n).collect(),
        None => vec![folder.to_path_buf()],
    };
    dirs.reverse();
    dirs
}

/// The project entries for `folder` ([`project_folders`]): `.claude/skills`
/// then `.claude/commands` of each, the repository root's first. A
/// `.claude` that is the account's config dir (a project in the home
/// folder) is left out: those are personal.
pub fn project(folder: &Path, config_dir: &Path) -> Vec<SlashCommand> {
    let config = fs::canonicalize(config_dir).ok();
    let mut skills = Vec::new();
    let mut commands = Vec::new();
    for dir in project_folders(folder) {
        let claude = dir.join(".claude");
        if config.is_some() && fs::canonicalize(&claude).ok() == config {
            continue;
        }
        scan_skills(
            &claude.join("skills"),
            None,
            &Origin::Project,
            None,
            &mut skills,
        );
        scan_commands(
            &claude.join("commands"),
            None,
            &Origin::Project,
            None,
            &mut commands,
        );
    }
    skills.extend(commands);
    skills
}

// ---- plugins -------------------------------------------------------------------------

/// An enabled plugin from `plugin list --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInstall {
    /// `id`, e.g. `review-kit@example-market`.
    pub id: String,
    /// The part of the id before `@`: the namespace when the manifest
    /// names none.
    pub name: String,
    /// The folder the plugin loads from (`readFromFolder`, else
    /// `installPath`): absolute and an existing folder.
    pub root: PathBuf,
}

/// The enabled plugins in the output of `plugin list --json` run in
/// `folder` (lenient, like the session listing; lines before the JSON are
/// skipped): an array of objects (or
/// the `installed` array of `--available`). An entry counts when its `id`
/// is a string, `enabled` is `true` and its folder exists; a `project` or
/// `local` install whose `projectPath` is another folder is left out, as is
/// `session` scope. Each folder once, at most [`MAX_PLUGINS`].
pub fn parse_plugin_list(text: &str, folder: &Path) -> Vec<PluginInstall> {
    // The JSON, after any line printed before it.
    let start = text
        .lines()
        .scan(0usize, |at, line| {
            let here = *at;
            *at += line.len() + 1;
            Some((here, line))
        })
        .find(|(_, line)| line.trim_start().starts_with(['[', '{']))
        .map_or(0, |(at, _)| at);
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text[start..].trim()) else {
        return Vec::new();
    };
    let items = match &value {
        serde_json::Value::Array(items) => items.as_slice(),
        serde_json::Value::Object(map) => match map.get("installed") {
            Some(serde_json::Value::Array(items)) => items.as_slice(),
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    let here = fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
    let mut out: Vec<PluginInstall> = Vec::new();
    for item in items {
        let str_of = |k: &str| item.get(k).and_then(serde_json::Value::as_str);
        let Some(id) = str_of("id") else {
            continue;
        };
        if item.get("enabled").and_then(serde_json::Value::as_bool) != Some(true) {
            continue;
        }
        let scope = str_of("scope").unwrap_or("");
        if scope == "session" {
            continue;
        }
        if matches!(scope, "project" | "local")
            && let Some(path) = str_of("projectPath")
        {
            let path = fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
            if path != here {
                continue;
            }
        }
        let root = str_of("readFromFolder")
            .filter(|p| !p.is_empty())
            .or_else(|| str_of("installPath"));
        let Some(root) = root.map(PathBuf::from) else {
            continue;
        };
        if !root.is_absolute() || !root.is_dir() {
            continue;
        }
        if out.iter().any(|p| p.root == root) {
            continue;
        }
        let name = id.split('@').next().unwrap_or(id).to_owned();
        out.push(PluginInstall {
            id: id.to_owned(),
            name,
            root,
        });
        if out.len() >= MAX_PLUGINS {
            break;
        }
    }
    out
}

/// A plugin path from the manifest: relative to the root, `./…` or `.`,
/// without `..` or a backslash.
fn manifest_path(root: &Path, p: &str) -> Option<PathBuf> {
    if p.contains('\\') || p.split('/').any(|s| s == "..") {
        return None;
    }
    let rel = match p {
        "." | "./" => "",
        _ => p.strip_prefix("./")?,
    };
    Some(root.join(rel))
}

/// One path or an array of paths, as the manifest's `skills` and
/// `commands` keys take them.
fn paths_of(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// The plugin manifest, `.claude-plugin/plugin.json`, when it is there,
/// small enough and JSON.
fn manifest(root: &Path) -> Option<serde_json::Value> {
    let path = root.join(".claude-plugin").join("plugin.json");
    if !contained(&path, fs::canonicalize(root).ok().as_deref()) {
        return None;
    }
    let meta = fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&path)
        .ok()?;
    let mut text = String::new();
    file.take(MAX_MANIFEST_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(&text).ok()
}

/// The entries of the plugin at `root` (its folder from the listing),
/// namespaced by the manifest's `name`, else `fallback` (the id's name):
///
/// - skills: `skills/<name>/SKILL.md`, plus the folders the manifest's
///   `skills` key adds (each a folder of skill folders or one holding a
///   `SKILL.md`); a `SKILL.md` at the root counts as one skill when there
///   is neither a `skills/` folder nor a `skills` key;
/// - commands: `commands/**/*.md`, or instead what the manifest's
///   `commands` key names: files, folders, or a map of name to `source`
///   / `content` with `description` and `argumentHint`.
///
/// Links are followed only while they stay inside the plugin's folder.
pub fn plugin(root: &Path, fallback: &str) -> Vec<SlashCommand> {
    let Ok(canon) = fs::canonicalize(root) else {
        return Vec::new();
    };
    let manifest = manifest(root);
    let get = |k: &str| manifest.as_ref().and_then(|m| m.get(k));
    let name = get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|n| valid_plugin_name(n))
        .unwrap_or(fallback)
        .to_owned();
    if !valid_plugin_name(&name) {
        return Vec::new();
    }
    let origin = Origin::Plugin(name.clone());
    let within = Some(canon.as_path());
    let mut skills = Vec::new();
    let default_skills = root.join("skills");
    scan_skills(&default_skills, Some(&name), &origin, within, &mut skills);
    let extra = get("skills").map(paths_of).unwrap_or_default();
    let default_canon = fs::canonicalize(&default_skills).ok();
    for p in &extra {
        let Some(dir) = manifest_path(root, p) else {
            continue;
        };
        if !dir.is_dir() || !contained(&dir, within) {
            continue;
        }
        if fs::canonicalize(&dir).ok() == default_canon && default_canon.is_some() {
            continue;
        }
        let file = dir.join("SKILL.md");
        if file.is_file() {
            let folder_name = dir
                .file_name()
                .and_then(|n| n.to_str())
                .map_or_else(|| name.clone(), str::to_owned);
            if let Some(head) = read_head(&file)
                && let Some(c) = entry(&head, &folder_name, true, Some(&name), &origin, Kind::Skill)
            {
                skills.push(c);
            }
        } else {
            scan_skills(&dir, Some(&name), &origin, within, &mut skills);
        }
    }
    if extra.is_empty() && !default_skills.exists() {
        let file = root.join("SKILL.md");
        if contained(&file, within)
            && let Some(head) = read_head(&file)
        {
            let folder_name = root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&name)
                .to_owned();
            if let Some(c) = entry(&head, &folder_name, true, Some(&name), &origin, Kind::Skill) {
                skills.push(c);
            }
        }
    }

    let mut commands = Vec::new();
    match get("commands") {
        None => scan_commands(
            &root.join("commands"),
            Some(&name),
            &origin,
            within,
            &mut commands,
        ),
        Some(serde_json::Value::Object(map)) => {
            for (key, spec) in map {
                if commands.len() >= MAX_COMMANDS {
                    break;
                }
                if let Some(c) = mapped_command(root, within, &name, &origin, key, spec) {
                    commands.push(c);
                }
            }
        }
        Some(value) => {
            for p in paths_of(value) {
                let Some(path) = manifest_path(root, &p) else {
                    continue;
                };
                if !contained(&path, within) {
                    continue;
                }
                if path.is_dir() {
                    scan_commands(&path, Some(&name), &origin, within, &mut commands);
                    continue;
                }
                let stem = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_suffix(".md"));
                if let Some(stem) = stem
                    && let Some(head) = read_head(&path)
                    && let Some(c) = entry(&head, stem, false, Some(&name), &origin, Kind::Command)
                {
                    commands.push(c);
                }
            }
        }
    }
    // A skill wins over a command of the same name.
    let mut seen = HashSet::new();
    skills
        .into_iter()
        .chain(commands)
        .filter(|c| seen.insert(c.name.clone()))
        .collect()
}

/// One entry of the manifest's `commands` map: `key` is the name after
/// the plugin prefix; `description` and `argumentHint` from the entry,
/// else from the frontmatter of its `source` file or its `content`.
fn mapped_command(
    root: &Path,
    within: Option<&Path>,
    plugin: &str,
    origin: &Origin,
    key: &str,
    spec: &serde_json::Value,
) -> Option<SlashCommand> {
    let str_of = |k: &str| spec.get(k).and_then(serde_json::Value::as_str);
    let head = if let Some(source) = str_of("source") {
        let path = manifest_path(root, source)?;
        if !contained(&path, within) {
            return None;
        }
        read_head(&path)?
    } else {
        parse_head(str_of("content")?)?
    };
    let mut c = entry(&head, key, false, Some(plugin), origin, Kind::Command)?;
    if let Some(d) = str_of("description").map(cap).filter(|d| !d.is_empty()) {
        c.description = d;
    }
    if let Some(h) = str_of("argumentHint").map(cap).filter(|h| !h.is_empty()) {
        c.argument_hint = Some(h);
    }
    Some(c)
}

// ---- the menu --------------------------------------------------------------------------

/// The list a menu shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Commands {
    /// Nothing asked for (no account or folder yet).
    #[default]
    None,
    /// The first load for this account and folder runs.
    Loading,
    /// The list (possibly from an earlier load while a refresh runs).
    Ready(Arc<CommandList>),
}

/// The `/` word the menu completes: on `row`, chars `start` (the `/`) to
/// `end` (exclusive), and what follows the `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub query: String,
}

/// The `/` word of `text`, when `/` is the first non-blank character of
/// the whole text and the cursor is inside that word (after the `/`, up to
/// its end). A word with a second `/` (a path) is not a command.
pub fn trigger(text: &TextArea) -> Option<Trigger> {
    let (row, start) = (0..text.line_count()).find_map(|r| {
        text.line(r)
            .iter()
            .position(|c| !c.is_whitespace())
            .map(|c| (r, c))
    })?;
    let line = text.line(row);
    if line[start] != '/' {
        return None;
    }
    let end = line[start..]
        .iter()
        .position(|c| c.is_whitespace())
        .map_or(line.len(), |n| start + n);
    let (crow, ccol) = text.cursor();
    if crow != row || ccol <= start || ccol > end {
        return None;
    }
    let query: String = line[start + 1..end].iter().collect();
    if query.contains('/') {
        return None;
    }
    Some(Trigger {
        row,
        start,
        end,
        query,
    })
}

/// The entries of `list` matching `query` (ignoring case): names that
/// start with it, then names whose part after a `:` starts with it, then
/// names that contain it; each group in list order. All for an empty
/// query.
pub fn matches<'a>(list: &'a CommandList, query: &str) -> Vec<&'a SlashCommand> {
    let q = query.to_lowercase();
    let mut ranked: Vec<(u8, &SlashCommand)> = list
        .commands
        .iter()
        .filter_map(|c| {
            let name = c.name.to_lowercase();
            if name.starts_with(&q) {
                Some((0, c))
            } else if name.split(':').skip(1).any(|s| s.starts_with(&q)) {
                Some((1, c))
            } else if name.contains(&q) {
                Some((2, c))
            } else {
                None
            }
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, c)| c).collect()
}

/// What the menu shows now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuView<'a> {
    /// `loading commands…`.
    Loading,
    /// The matching entries and the chosen one (an index into them).
    List {
        items: Vec<&'a SlashCommand>,
        selected: usize,
    },
}

/// The suggestion menu of a text field (doc/0042). See the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlashMenu {
    key: Option<CommandKey>,
    commands: Commands,
    selected: usize,
    query: Option<String>,
    dismissed: Option<String>,
    /// The entry accepted last: its name and argument hint.
    accepted: Option<(String, String)>,
}

/// The text the menu's loading row shows.
pub const LOADING_TEXT: &str = "loading commands…";

impl SlashMenu {
    /// The account and folder the list is for.
    pub fn key(&self) -> Option<&CommandKey> {
        self.key.as_ref()
    }

    /// The list, as far as it is known.
    pub fn commands(&self) -> &Commands {
        &self.commands
    }

    /// Points the menu at the list of `key` (the app's cache gives what it
    /// has).
    pub fn set_source(&mut self, key: Option<CommandKey>, commands: Commands) {
        self.key = key;
        self.commands = commands;
        self.selected = 0;
    }

    /// Takes a finished load for the current key.
    pub fn set_commands(&mut self, commands: Commands) {
        self.commands = commands;
    }

    /// What the menu shows for `text`, or `None` while it is closed: no
    /// `/` word at the cursor, `Esc` pressed for this word, nothing asked
    /// for, an empty list or no match.
    pub fn view<'a>(&'a self, text: &TextArea) -> Option<MenuView<'a>> {
        let t = trigger(text)?;
        if self.dismissed.as_deref() == Some(t.query.as_str()) {
            return None;
        }
        match &self.commands {
            Commands::None => None,
            Commands::Loading => Some(MenuView::Loading),
            Commands::Ready(list) => {
                let items = matches(list, &t.query);
                if items.is_empty() {
                    return None;
                }
                let selected = self.selected.min(items.len() - 1);
                Some(MenuView::List { items, selected })
            }
        }
    }

    /// Handles a key while the field has the focus; `true` when the menu
    /// took it. Open with entries: `↑` / `↓` choose (wrapping), `Tab` and
    /// `Enter` accept, `Esc` closes the menu. Loading: only `Esc`. Every
    /// other key (and these with the menu closed) is the field's.
    pub fn handle_key(&mut self, text: &mut TextArea, key: &KeyEvent) -> bool {
        let Some(view) = self.view(text) else {
            return false;
        };
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        if key.code == KeyCode::Esc {
            self.dismissed = trigger(text).map(|t| t.query);
            return true;
        }
        let MenuView::List { items, selected } = view else {
            return false;
        };
        let n = items.len();
        match key.code {
            KeyCode::Up if plain => {
                self.selected = (selected + n - 1) % n;
                true
            }
            KeyCode::Down if plain => {
                self.selected = (selected + 1) % n;
                true
            }
            KeyCode::Tab | KeyCode::Enter if plain => {
                let chosen = items[selected].clone();
                self.accept(text, &chosen);
                true
            }
            _ => false,
        }
    }

    /// Replaces the `/` word with `/name ` and closes the menu (the cursor
    /// ends after the space, outside the word).
    fn accept(&mut self, text: &mut TextArea, chosen: &SlashCommand) {
        let Some(t) = trigger(text) else {
            return;
        };
        text.replace_range(t.row, t.start, t.end, &format!("/{}", chosen.name));
        let (row, col) = text.cursor();
        if text.line(row).get(col) == Some(&' ') {
            text.set_cursor(row, col + 1);
        } else {
            text.insert_text(" ");
        }
        self.accepted = chosen
            .argument_hint
            .clone()
            .map(|h| (chosen.name.clone(), h));
        self.selected = 0;
        self.sync(text);
    }

    /// After a key or an edit: a changed `/` word starts again at the top
    /// and lifts an `Esc`.
    pub fn sync(&mut self, text: &TextArea) {
        let query = trigger(text).map(|t| t.query);
        if query != self.query {
            self.selected = 0;
            self.query = query.clone();
        }
        if self.dismissed.is_some() && self.dismissed != query {
            self.dismissed = None;
        }
    }

    /// The accepted entry's `/name` and argument hint, while the text
    /// still starts with that command.
    pub fn argument_hint(&self, text: &TextArea) -> Option<(String, String)> {
        let (name, hint) = self.accepted.as_ref()?;
        let all = text.text();
        let first = all.split_whitespace().next()?;
        (first.strip_prefix('/') == Some(name.as_str())).then(|| (format!("/{name}"), hint.clone()))
    }
}
