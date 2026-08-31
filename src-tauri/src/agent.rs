//! What each agent CLI understands.
//!
//! Everything this app does to a session beyond "open a terminal and get out
//! of the way" is a convention of one particular CLI: which argument carries
//! the first prompt, which flag picks the conversation back up, how a
//! permission mode is spelled, where the status hooks are configured, and how
//! the token account is written down. Each of those used to be an
//! `if agent == "claude"` somewhere far from the others, and the cost showed:
//! a second CLI meant auditing the whole core for the places that had quietly
//! assumed the first.
//!
//! So they are gathered here, one table, and the rest of the core asks the
//! table. A CLI that is not in it is not broken — it opens a terminal like
//! any other, and every layer on top says plainly that it does not apply.
//!
//! Nothing in here is a guess. Claude Code's half was measured against the
//! real CLI (`tests/prompt_injection.rs`, `tests/hooks.rs`); Codex's half is
//! taken from its published reference and is held to it by
//! `.github/workflows/agent-parity.yml`, which installs both CLIs and checks
//! that every flag named below is one the installed binary actually accepts.
//! A documented flag that stops being real fails there, on a schedule, rather
//! than in front of somebody's session.

use crate::hooks;
use crate::store::PermissionMode;

/// The CLIs whose conventions are known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cli {
    Claude,
    Codex,
}

/// How a CLI is told to pick a conversation back up.
///
/// The distinction is not cosmetic. `--continue` is an option and may sit
/// anywhere among the others; `resume` is a subcommand and everything that
/// modifies the run has to come *before* it. Spelling that out here is what
/// stops the command line being assembled in an order one of them refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// An option among the others: `claude --continue`.
    Option(&'static [&'static str]),
    /// A subcommand, which every option must precede: `codex resume --last`.
    Subcommand(&'static [&'static str]),
}

/// How a CLI writes down what a turn cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ledger {
    /// One row per assistant message, each carrying that message's own
    /// usage. The account is the **sum** of the rows.
    PerMessage,
    /// One row per token count, each carrying the running total for the
    /// whole session. The account is the **last** row, and adding them up
    /// would multiply the bill by the number of turns.
    Cumulative,
}

/// The Codex release whose hooks engine is the one described in its
/// reference — all six events this app reports on, the stdin payload, and
/// the per-hook timeout.
///
/// Handed to an older Codex the `-c hooks.*` overrides are config keys it
/// does not know, and what happens then is its business, not ours: unknown
/// means the hooks stay off and the session runs without status, which is
/// exactly where a Codex session was before any of this.
pub const CODEX_HOOKS_SINCE: (u64, u64, u64) = (0, 124, 0);

/// What each CLI is told to update itself with.
///
/// Both spell it `update`, and both mean the same thing by it: work out how
/// this copy was installed and run that install method's upgrade. That is
/// the whole reason the command is theirs and not ours. A desk that tried to
/// update them itself would have to tell an npm global from a native install
/// from a Homebrew cask from an apt package — and then be wrong in the one
/// way that matters, because `npm install -g` over a native install does not
/// replace it, it adds a second one and leaves which `claude` runs to
/// whichever directory PATH names first.
///
/// It is also the check. Neither offers a look-without-touching mode —
/// Codex's `update` takes no flags at all — so "is there a new one" and
/// "get it" are one command, and an already-current CLI answers by saying so.
pub const UPDATE_SUBCOMMAND: &str = "update";

impl Cli {
    /// Which CLI a launcher's resolved agent name is, or `None` for one
    /// whose conventions nobody has measured.
    pub fn of(agent: &str) -> Option<Self> {
        match agent {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The flags a permission mode adds to a launch.
    ///
    /// `Normal` is empty for both on purpose: it means "the CLI's own
    /// defaults", and the honest way to say that is to pass nothing.
    ///
    /// The two spellings are not translations of each other, because the
    /// CLIs do not divide the ground the same way. Claude Code has a
    /// permission mode; Codex has a sandbox and an approval policy, and
    /// "edits go through, commands still ask" is the pair
    /// `workspace-write` + `on-request` — writes inside the worktree need
    /// no approval, anything that reaches past it still does.
    pub fn mode_args(self, mode: PermissionMode) -> &'static [&'static str] {
        match (self, mode) {
            (_, PermissionMode::Normal) => &[],
            (Self::Claude, PermissionMode::AcceptEdits) => &["--permission-mode", "acceptEdits"],
            (Self::Claude, PermissionMode::Yolo) => &["--dangerously-skip-permissions"],
            (Self::Codex, PermissionMode::AcceptEdits) => &[
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
            ],
            (Self::Codex, PermissionMode::Yolo) => &["--dangerously-bypass-approvals-and-sandbox"],
        }
    }

    /// How this CLI resumes the conversation already in the directory.
    ///
    /// Both are cwd-scoped, which is the property the whole thing rests on:
    /// an attempt's worktree path never changes, so "the conversation in
    /// here" is unambiguous even after the app has been restarted.
    pub fn resume(self) -> Resume {
        match self {
            Self::Claude => Resume::Option(&["--continue"]),
            Self::Codex => Resume::Subcommand(&["resume", "--last"]),
        }
    }

    pub fn ledger(self) -> Ledger {
        match self {
            Self::Claude => Ledger::PerMessage,
            Self::Codex => Ledger::Cumulative,
        }
    }

    /// The version from which this app's hook wiring is known to apply, or
    /// `None` when it has always applied.
    ///
    /// `--plugin-dir` predates every Claude Code this app has ever run
    /// against, so there is nothing to gate. Codex's hooks engine is
    /// younger than Codex.
    pub fn hooks_since(self) -> Option<(u64, u64, u64)> {
        match self {
            Self::Claude => None,
            Self::Codex => Some(CODEX_HOOKS_SINCE),
        }
    }

    /// Whether a CLI of this version can be wired for status.
    ///
    /// Unknown is a no, in the direction that never breaks a session: an
    /// unrecognised flag is the one failure a person cannot work around from
    /// inside the terminal, because there is no terminal — the CLI exited
    /// before it drew one.
    pub fn hooks_ok(self, version: Option<(u64, u64, u64)>) -> bool {
        match self.hooks_since() {
            None => true,
            Some(since) => version >= Some(since),
        }
    }

    /// The arguments that point this CLI at our status reporting.
    ///
    /// Two different mechanisms, for a reason that is theirs rather than
    /// ours. Claude Code loads a plugin directory, so the hooks live in a
    /// file this app wrote and the launch only names the folder. Codex has
    /// no per-launch plugin flag: hooks come from config, and the only place
    /// to put config for one launch without editing a file somebody else
    /// owns is `-c`, whose value it parses as TOML.
    ///
    /// Neither writes into the user's own configuration. That is the whole
    /// point of both — an app that injects itself into `~/.claude/settings.json`
    /// or `~/.codex/config.toml` is an app that can silently disable the
    /// hooks somebody wrote for themselves.
    pub fn hook_args(self, wiring: &hooks::Wiring) -> Vec<String> {
        match self {
            Self::Claude => vec!["--plugin-dir".to_string(), wiring.plugin_dir.clone()],
            Self::Codex => hooks::codex_config_args(&wiring.url),
        }
    }

    /// Every flag this app may put in front of this CLI, for the parity
    /// workflow to hold the installed binary to.
    ///
    /// Derived from the same functions the launch path calls, not typed out
    /// beside them: a list maintained by hand is a list that goes stale the
    /// first time somebody adds a flag, and a stale list is worse than none
    /// because it reports green.
    pub fn every_flag(self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for mode in [
            PermissionMode::Normal,
            PermissionMode::AcceptEdits,
            PermissionMode::Yolo,
        ] {
            out.extend(self.mode_args(mode).iter().filter(|a| a.starts_with('-')));
        }
        // Only a resume that *is* an option. A subcommand's own flags live on
        // its own help page, and looking for them on the front one passes
        // today by luck — `codex --help` happens to mention `--last` in the
        // sentence describing `resume` — and would fail the day that sentence
        // is reworded, for no reason anybody could act on.
        if let Resume::Option(words) = self.resume() {
            out.extend(words.iter().filter(|w| w.starts_with('-')));
        }
        out.extend(match self {
            Self::Claude => ["--plugin-dir", "--name"].as_slice(),
            // `-c` is the only one the wiring adds; its value is config, not
            // a flag, and config a CLI does not know is config it ignores.
            Self::Codex => ["-c"].as_slice(),
        });
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Where a CLI keeps the instructions it reads before anyone types, as
/// slots rather than discoveries — a rules file that is absent is still
/// worth naming, because "it goes here" is the answer people came for.
///
/// `shared` is `AGENTS.md`, the one file every one of them agreed to look
/// at. It belongs to no single CLI, which is why it is named that way and
/// not attributed to whichever one happens to be running.
pub struct Docs {
    /// `(filename, which CLI reads it)` in the checkout.
    pub project_rules: &'static [(&'static str, &'static str)],
    /// `(home subdirectory, filename, which CLI reads it)`.
    pub global_rules: &'static [(&'static str, &'static str, &'static str)],
    /// `(home/project subdirectory holding one directory per skill, which
    /// CLI reads it)`. Both look for a `SKILL.md` inside each.
    pub skill_roots: &'static [(&'static str, &'static str)],
}

/// Every CLI's conventions, not only the running one's.
///
/// This is the rare surface where the agents are equal, and narrowing it to
/// whichever one this session happens to run would throw that away for
/// nothing: the question "where do the conventions for this repo go" is
/// asked about the repository, not about the session.
pub const DOCS: Docs = Docs {
    project_rules: &[
        ("CLAUDE.md", "claude"),
        ("AGENTS.md", "shared"),
        ("GEMINI.md", "gemini"),
    ],
    global_rules: &[
        (".claude", "CLAUDE.md", "claude"),
        (".codex", "AGENTS.md", "codex"),
        (".gemini", "GEMINI.md", "gemini"),
    ],
    skill_roots: &[(".claude", "claude"), (".codex", "codex")],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_clis_whose_conventions_are_measured_are_in_the_table() {
        assert_eq!(Cli::of("claude"), Some(Cli::Claude));
        assert_eq!(Cli::of("codex"), Some(Cli::Codex));
        for other in ["gemini", "aider", "something-new", ""] {
            assert_eq!(Cli::of(other), None, "{other}");
        }
    }

    /// The property every permission mode has to keep, in both spellings:
    /// asking less is a choice a person makes, so a mode must never ask for
    /// *more* than the one below it, and `normal` must add nothing at all.
    #[test]
    fn normal_means_the_clis_own_defaults_and_nothing_else() {
        for cli in [Cli::Claude, Cli::Codex] {
            assert!(
                cli.mode_args(PermissionMode::Normal).is_empty(),
                "{} adds flags to its own defaults",
                cli.name()
            );
            for mode in [PermissionMode::AcceptEdits, PermissionMode::Yolo] {
                assert!(
                    !cli.mode_args(mode).is_empty(),
                    "{} has nothing to say for {:?}",
                    cli.name(),
                    mode
                );
            }
        }
    }

    /// A resume must not carry a prompt, and a subcommand must not be
    /// mistaken for an option — `codex --continue` is not a thing, and
    /// `claude resume` is a directory called resume.
    #[test]
    fn each_cli_resumes_the_way_it_actually_spells_it() {
        assert_eq!(Cli::Claude.resume(), Resume::Option(&["--continue"]));
        assert_eq!(Cli::Codex.resume(), Resume::Subcommand(&["resume", "--last"]));
    }

    /// The gate that keeps `-c hooks.*` off a Codex that predates the hooks
    /// engine. Unknown stays off, the direction that never costs a session.
    #[test]
    fn codex_hooks_are_gated_on_a_version_and_claude_needs_no_gate() {
        assert!(Cli::Claude.hooks_ok(None), "claude has no gate to fail");
        assert!(!Cli::Codex.hooks_ok(None), "unknown must stay off");
        assert!(!Cli::Codex.hooks_ok(Some((0, 123, 999))));
        assert!(Cli::Codex.hooks_ok(Some(CODEX_HOOKS_SINCE)));
        assert!(Cli::Codex.hooks_ok(Some((1, 0, 0))));
    }

    /// The list the parity workflow holds a real CLI to. It has to come from
    /// the launch path's own functions, or it reports green on flags nothing
    /// passes any more.
    #[test]
    fn every_flag_is_gathered_from_the_functions_that_pass_them() {
        let claude = Cli::Claude.every_flag();
        for f in ["--permission-mode", "--dangerously-skip-permissions", "--continue", "--plugin-dir", "--name"] {
            assert!(claude.contains(&f), "claude's list lost {f}: {claude:?}");
        }
        let codex = Cli::Codex.every_flag();
        for f in ["--sandbox", "--ask-for-approval", "--dangerously-bypass-approvals-and-sandbox", "-c"] {
            assert!(codex.contains(&f), "codex's list lost {f}: {codex:?}");
        }
        // Values are not flags. `acceptEdits` and `workspace-write` are
        // arguments to the flag before them, and asking a `--help` whether it
        // mentions them would be asking the wrong question.
        for list in [&claude, &codex] {
            for f in list {
                assert!(f.starts_with('-'), "{f} is a value, not a flag");
            }
        }
    }

    /// The frontend keeps its own copy of this list, because half a dozen
    /// controls turn on it and none of them can call in here. A copy that
    /// drifts does not break anything loudly — it offers a permission mode
    /// for a CLI that has none, or hides a send button from one that would
    /// have taken it — so the two are checked against each other rather than
    /// trusted to stay in step.
    ///
    /// Both files are read as text on purpose: the point is to fail when
    /// somebody adds a CLI here and forgets there, and any check clever
    /// enough to be robust to reformatting would be clever enough to miss
    /// that.
    #[test]
    fn the_frontends_copy_of_this_table_names_the_same_clis() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the repository root");
        for (rel, marker) in [
            ("ui/src/agents.ts", "MEASURED_AGENTS = ["),
            // The Playwright mock refuses a follow-up the same way the core
            // does; a mock that is more generous than the product turns a
            // real refusal into a test that never sees it.
            ("ui/tests/mock-tauri.ts", "const measured = "),
        ] {
            let path = root.join(rel);
            let Ok(text) = std::fs::read_to_string(&path) else {
                // A backend-only checkout (the crate vendored elsewhere) is
                // not a failing checkout; it just cannot run this check.
                eprintln!("skip {rel}: not in this checkout");
                continue;
            };
            let line = text
                .lines()
                .find(|l| l.contains(marker))
                .unwrap_or_else(|| panic!("{rel} no longer contains `{marker}`"));
            for cli in [Cli::Claude, Cli::Codex] {
                assert!(
                    line.contains(cli.name()),
                    "{rel} does not list `{}`: {line}",
                    cli.name()
                );
            }
            for absent in ["gemini", "aider"] {
                assert!(
                    !line.contains(absent),
                    "{rel} claims `{absent}` is measured: {line}"
                );
            }
        }
    }

    /// Both CLIs' rules files are listed for every repository, whichever one
    /// is running. Dropping the ones the running agent does not read would
    /// make the tab answer a narrower question than the one people open it
    /// with.
    #[test]
    fn the_knows_table_names_every_clis_conventions_not_the_running_ones() {
        let project: Vec<_> = DOCS.project_rules.iter().map(|(n, _)| *n).collect();
        assert!(project.contains(&"CLAUDE.md"));
        assert!(project.contains(&"AGENTS.md"), "codex's project rules file");
        let skills: Vec<_> = DOCS.skill_roots.iter().map(|(d, _)| *d).collect();
        assert_eq!(skills, vec![".claude", ".codex"]);
    }
}
