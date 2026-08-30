//! Git worktrees, one per repository per attempt.
//!
//! An attempt is a go at a card with one agent, and it gets its own worktree
//! and its own branch so two agents can work the same repository at the same
//! time without seeing each other's files. The base commit is recorded when
//! the worktree opens, because that — not `main` as it stands later — is what
//! the attempt's diff is against.
//!
//! A card may name more than one repository, because a change that has to
//! land in a service and its client is one piece of work and one
//! conversation. Then the attempt gets a *root directory* holding one
//! worktree per repository, side by side, and the session starts in that
//! directory rather than in any one checkout. The safety argument is
//! unchanged and that is the point: every repository the agent can reach is
//! still a worktree on a branch of this attempt's own, and none of them is
//! the person's checkout. Nothing here can spend anything but its own
//! branches — there are simply several of them now.
//!
//! Every git invocation goes through the repository's host and its login
//! environment, for the same reason sessions do: the user's git, the user's
//! git config, and later the credentials and SSH agent that `git push` and
//! `gh` need. A GUI process's own environment has none of that — and for a
//! repository inside WSL, the git that owns it is the distro's, not ours.
//!
//! Paths in here are strings in the host's own spelling, never `PathBuf`:
//! on Windows a `PathBuf` joins with backslashes, which would quietly corrupt
//! a POSIX path inside a distro.

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::host::{Host, HostRef};
use crate::i18n::{self, Locale};

/// Longest slug taken from a title. Long enough to recognise the card in
/// `git branch`, short enough that the worktree path stays readable.
const MAX_SLUG: usize = 32;

/// One repository an attempt is to be opened in, as the card names it.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoSpec {
    /// The path inside the host, already located.
    pub repo: String,
    pub base_branch: String,
}

/// One checkout an attempt opened, in host-side paths.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedTree {
    pub repo: String,
    pub base_branch: String,
    /// Its directory inside the attempt's root, and so the prefix its paths
    /// wear in the diff. Empty when the card names one repository: the root
    /// *is* the checkout, and its diff paths are already what a person would
    /// type.
    pub dir: String,
    pub path: String,
    pub branch: String,
    pub base_sha: String,
}

/// Where a newly opened attempt lives, in host-side paths.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedWorktree {
    /// Where the session starts: the checkout itself for one repository, the
    /// directory holding them all for several.
    pub root: String,
    pub trees: Vec<OpenedTree>,
    /// The branch every checkout is on — one name across the repositories,
    /// because they are one piece of work.
    pub branch: String,
    /// Which attempt number this turned out to be. May be higher than asked
    /// for, if git already had branches in the way.
    pub seq: i64,
}

impl OpenedWorktree {
    /// The first repository's checkout — the one the card's badge names and
    /// the one an attempt row records. Always present: an attempt with no
    /// tree is refused before it is opened.
    pub fn first(&self) -> &OpenedTree {
        &self.trees[0]
    }
}

pub struct Worktrees {
    root: PathBuf,
}

/// Turn a card title into something that can be a branch name and a directory.
///
/// Titles here are commonly Chinese, and a title with no ASCII in it at all
/// slugifies to nothing — `marol/-1` is not a valid branch and `-1` is a
/// directory name that reads as a flag to half the tools that would touch it.
/// So an empty result falls back to the task id, which is always usable and
/// still identifies the card.
pub fn slug(title: &str, task_id: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true; // leading dashes are never wanted
    for ch in title.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && out.len() < MAX_SLUG {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= MAX_SLUG {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        // Short prefix of the id: enough to tell two cards apart, short
        // enough to stay readable in `git branch`.
        let short: String = task_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
        return format!("task-{short}");
    }
    out
}

/// FNV-1a. Two checkouts of different repositories often share a folder name
/// (`api`, `web`), so the directory that holds a repository's worktrees is
/// keyed by its full path, not just its last component.
fn path_hash(p: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in p.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:08x}")[..8].to_string()
}

/// The record separator these batched scripts put between their sections.
///
/// U+001E on a line of its own: it is the ASCII character whose entire job is
/// this, and no git output contains it — a marker made of printable text
/// would eventually appear inside a diff and split it in half.
const SEP: &str = "\u{1e}";

/// Run one script inside the host and hand back its sections.
///
/// The shape every batched read here takes: a shell script that runs several
/// commands in the world the repository lives in and prints a separator
/// between them, so what used to cost one process per question costs one
/// process per *answer*. The count is checked rather than assumed — a script
/// that died half way through would otherwise hand back a short list and let
/// the caller read section two as though it were section three.
fn sh_sections(
    hr: &HostRef,
    cwd: &str,
    script: &str,
    args: &[&str],
    want: usize,
    doing: &str,
) -> Result<Vec<String>> {
    debug_assert!(
        !matches!(hr.host, Host::Local),
        "the batched scripts are for crossings; locally there is no doorway and no `sh` \
         to rely on — see `batching_is_for_doorways_only`"
    );
    let mut argv: Vec<&str> = vec!["-c", script, "_"];
    argv.extend(args.iter().copied());
    let out = hr
        .run("sh", &argv, Some(cwd))
        .with_context(|| format!("{doing} in {cwd}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "{doing} in {cwd} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    // Split on the character alone, not on a newline-wrapped marker: a
    // section that produced nothing at all leaves the separator flush against
    // the start of the output, where a `\n<SEP>\n` pattern would not match
    // and every later section would be read as the wrong one. The stray
    // newlines that survive are line noise to every caller here, all of which
    // read their section a line at a time.
    let sections: Vec<String> = text.split(SEP).map(str::to_string).collect();
    if sections.len() != want {
        return Err(anyhow!(
            "{doing} in {cwd} answered with {} sections, not {want}",
            sections.len()
        ));
    }
    Ok(sections)
}

/// Everything `stat` needs, in the order `stat` reads it.
///
/// `$1` is the base commit, `$2` the base branch. Each section that used to
/// be a `git()?` still fails loudly, with an exit code of its own so the
/// error says which question went wrong rather than only that something did.
/// The untracked loop stays best-effort, exactly as the per-file call it
/// replaces was.
///
/// Filenames are read a line at a time, which is the same thing the code this
/// replaces did with `untracked.lines()` — a newline in a filename was
/// already outside what this counts, and moving the loop across the doorway
/// does not make it worse.
const STAT_SCRIPT: &str = r#"git diff --numstat "$1" || exit 11
printf '\036\n'
git ls-files --others --exclude-standard | while IFS= read -r f; do
  [ -n "$f" ] || continue
  git diff --no-index --numstat -- /dev/null "$f" || true
done
printf '\036\n'
git rev-list --left-right --count "$2...HEAD" || exit 13
printf '\036\n'
git status --porcelain || exit 14
"#;

/// The rendered diff, tracked then untracked, in one crossing.
///
/// `$1` is the base commit and `$2…` the `--src-prefix`/`--dst-prefix` pair a
/// multi-repo workspace needs — shifted off so the loop can pass them on with
/// `"$@"`.
///
/// `--no-index` against /dev/null renders a new file as the patch that would
/// create it, so it reads like the rest of the diff; it exits 1 whenever
/// there is a difference, which there always is, hence `|| true`.
const DIFF_SCRIPT: &str = r#"base="$1"; shift
git diff "$@" "$base" || exit 11
printf '\036\n'
git ls-files --others --exclude-standard | while IFS= read -r f; do
  [ -n "$f" ] || continue
  git diff "$@" --no-index -- /dev/null "$f" || true
done
"#;

fn git(hr: &HostRef, cwd: &str, args: &[&str]) -> Result<String> {
    let out = hr
        .run("git", args, Some(cwd))
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed in {cwd}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether git already knows this branch. Checked before claiming a name,
/// because a branch outlives both the worktree that made it and the row that
/// recorded it: delete a card, make another with the same title, and the
/// numbering starts over onto branches that are still there.
fn branch_exists(hr: &HostRef, repo: &str, branch: &str) -> bool {
    hr.run(
        "git",
        &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")],
        Some(repo),
    )
    .map(|o| o.status.success())
    .unwrap_or(false)
}

/// The directory holding one repository's worktrees, under `root`.
fn dir_for(host: &Host, root: &str, repo: &str) -> String {
    host.join(root, &format!("{}-{}", repo_name(repo), path_hash(repo)))
}

/// What a repository is called: its own last path component.
fn repo_name(repo: &str) -> &str {
    repo.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or("repo")
}

/// A directory name inside the attempt's root for each repository, in order.
///
/// The repository's own name, because that is what the person calls it and
/// what the agent will type — and what the diff's paths will read as, since
/// they are rendered relative to the root. Two checkouts very often share a
/// name (`api`, `web`), so one already taken picks up the same path hash the
/// worktree directories are keyed by: still recognisable, and unique by
/// construction rather than by luck.
fn tree_dirs(repos: &[RepoSpec]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(repos.len());
    for r in repos {
        let name = repo_name(&r.repo);
        let taken = out.iter().any(|d| d == name);
        out.push(if taken {
            format!("{name}-{}", path_hash(&r.repo))
        } else {
            name.to_string()
        });
    }
    out
}

/// The directory names an attempt on these repositories *would* get, for the
/// prompt the start dialog shows before anything has been opened. One
/// repository gets no directory at all, which is what makes the preview and
/// the real thing say the same sentence.
pub fn preview_dirs(repos: &[RepoSpec]) -> Vec<String> {
    if repos.len() <= 1 {
        return vec![String::new(); repos.len()];
    }
    tree_dirs(repos)
}

/// The `--src-prefix`/`--dst-prefix` pair that renders one checkout's paths
/// relative to the attempt's root instead of to its own repository.
///
/// Empty for a one-repository attempt, so its diff is byte-for-byte the one
/// this app has always produced. For the rest it is what makes `web/api.ts`
/// and `api/routes.py` name two different files in one diff — and what lets
/// a review comment, and the in-place editor, point at a path the agent can
/// open from where it is standing.
fn prefix_args(dir: &str) -> Vec<String> {
    if dir.is_empty() {
        return Vec::new();
    }
    vec![
        format!("--src-prefix=a/{dir}/"),
        format!("--dst-prefix=b/{dir}/"),
    ]
}

/// `["diff", <prefixes>, ...rest]` as the borrowed slice `git` takes.
fn with_prefix<'a>(dir_args: &'a [String], rest: &[&'a str]) -> Vec<&'a str> {
    let mut out: Vec<&str> = vec!["diff"];
    out.extend(dir_args.iter().map(String::as_str));
    out.extend(rest.iter().copied());
    out
}

impl Worktrees {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `~/.marol/worktrees`.
    ///
    /// Not beside the repository, which is where this obviously belongs until
    /// you notice that a repository's parent directory is very often itself a
    /// repository — an umbrella workspace holding several checkouts. Putting
    /// worktrees there nests a repository inside another one, and every tool
    /// that walks upward looking for `.git` starts answering differently.
    ///
    /// Not in the application support directory either: these are working
    /// trees a person will want to `cd` into, open in an editor, and run
    /// builds from. A path they can type is worth more than tidiness.
    ///
    /// **The old root wins when it is there.** Unlike the state directory,
    /// these paths are not ours alone to move: each one is written into the
    /// attempt row that opened it, and registered inside its repository's
    /// `.git/worktrees/<id>/gitdir` with the worktree's own `.git` file
    /// pointing back. Moving the directory would break both ends of that and
    /// leave every open attempt pointing at nothing — a rename that reaches
    /// into the person's repositories to fix itself is not a rename any more.
    ///
    /// So a desk that has one goes on using it, under the name it was made
    /// with, for as long as it exists. Nothing is stranded and nothing is
    /// touched. New installs get `~/.marol/worktrees`, and so does this one
    /// once the last of the old trees is handed back.
    pub fn default_root() -> PathBuf {
        Self::default_root_in(&dirs::home_dir().unwrap_or_else(std::env::temp_dir))
    }

    /// The same choice, against a named home, so it can be asked without one.
    pub fn default_root_in(home: &Path) -> PathBuf {
        let former = home.join(".agentdesk").join("worktrees");
        if former.is_dir() {
            return former;
        }
        home.join(".marol").join("worktrees")
    }

    /// This app machine's own worktree root. A non-local host keeps its
    /// worktrees in its own filesystem — see `Core::host_env`.
    pub fn local_root(&self) -> String {
        self.root.to_string_lossy().to_string()
    }

    /// Where a branch points right now.
    pub fn head_of(&self, hr: &HostRef, repo: &str, branch: &str) -> Result<String> {
        git(hr, repo, &["rev-parse", branch])
    }

    /// Refuse a card that cannot produce a working attempt, at the moment it
    /// is created rather than when someone first tries to run it. Ad-hoc
    /// sessions are not subject to any of this — they are just a directory.
    pub fn check_repo(&self, hr: &HostRef, repo: &str, base_branch: &str) -> Result<()> {
        if !hr.is_dir(repo) {
            return Err(anyhow!("{repo} is not a directory"));
        }
        let inside = git(hr, repo, &["rev-parse", "--is-inside-work-tree"])
            .map_err(|_| anyhow!("{repo} is not a git repository"))?;
        if inside.trim() != "true" {
            return Err(anyhow!("{repo} is not a git repository"));
        }
        if !branch_exists(hr, repo, base_branch) {
            return Err(anyhow!("{repo} has no branch `{base_branch}`"));
        }
        Ok(())
    }

    /// Open an attempt's ground: one fresh branch, and a worktree on it in
    /// every repository the card names.
    ///
    /// The whole attempt lives under `root` — which is in the same host as
    /// the repositories, never on the app's side of a boundary. One
    /// repository puts its checkout straight at the attempt's path, exactly
    /// as this has always done; several put one directory each inside it,
    /// named after the repository, and the attempt's path becomes the
    /// workspace the session starts in.
    ///
    /// **One branch name, in every repository.** They are one piece of work
    /// and reviewing them under one name is the point; it also means the
    /// numbering has a single answer to walk forward from. `start_seq` is
    /// where that walk begins, and it goes past any number *any* of the
    /// repositories already has a branch for, so `marol/login-2` never means
    /// two different things in two checkouts of one attempt.
    ///
    /// A failure part-way through takes back what it already opened. A
    /// half-made workspace would look like a working one and diff as if the
    /// missing repository had no changes.
    pub fn create(
        &self,
        hr: &HostRef,
        root: &str,
        repos: &[RepoSpec],
        slug: &str,
        start_seq: i64,
    ) -> Result<OpenedWorktree> {
        let Some(first) = repos.first() else {
            return Err(anyhow!("an attempt needs at least one repository"));
        };
        for r in repos {
            self.check_repo(hr, &r.repo, &r.base_branch)?;
        }
        let solo = repos.len() == 1;

        // Keyed on the first repository, so every attempt at a card lands in
        // the same directory whatever else the card came to span.
        let dir = dir_for(hr.host, root, &first.repo);
        hr.mkdir_p(&dir)
            .with_context(|| format!("creating {dir}"))?;

        let mut seq = start_seq.max(1);
        let (attempt_root, branch) = loop {
            if seq > start_seq + 1000 {
                return Err(anyhow!("no free attempt number for `{slug}` after 1000 tries"));
            }
            let branch = format!("marol/{slug}-{seq}");
            let path = hr.join(&dir, &format!("{slug}-{seq}"));
            let free = !hr.exists(&path)
                && repos.iter().all(|r| !branch_exists(hr, &r.repo, &branch));
            if free {
                break (path, branch);
            }
            seq += 1;
        };

        let dirs = tree_dirs(repos);
        let mut trees: Vec<OpenedTree> = Vec::with_capacity(repos.len());
        for (r, d) in repos.iter().zip(dirs) {
            // The base as it stands right now. Recorded rather than
            // re-resolved later, because `main` keeps moving and the
            // attempt's diff has to stay against what it actually started
            // from.
            let base_sha = git(hr, &r.repo, &["rev-parse", &r.base_branch])?;
            let (dir, path) = if solo {
                (String::new(), attempt_root.clone())
            } else {
                let path = hr.join(&attempt_root, &d);
                (d, path)
            };
            trees.push(OpenedTree {
                repo: r.repo.clone(),
                base_branch: r.base_branch.clone(),
                dir,
                path,
                branch: branch.clone(),
                base_sha,
            });
        }

        // `git worktree add` makes the leaf; the workspace above it is ours.
        if !solo {
            hr.mkdir_p(&attempt_root)
                .with_context(|| format!("creating {attempt_root}"))?;
        }

        for (i, t) in trees.iter().enumerate() {
            let added = git(
                hr,
                &t.repo,
                &["worktree", "add", "-b", &t.branch, &t.path, &t.base_branch],
            );
            if let Err(e) = added {
                for done in &trees[..i] {
                    let _ = self.remove(hr, &done.repo, &done.path);
                }
                if !solo {
                    let _ = hr.remove_dir(&attempt_root);
                }
                return Err(e)
                    .with_context(|| format!("opening a worktree for `{}` in {}", t.branch, t.repo));
            }
        }

        Ok(OpenedWorktree {
            root: attempt_root,
            trees,
            branch,
            seq,
        })
    }

    /// Grow a worktree back onto an attempt's *existing* branch, at the
    /// exact path it had before — the resume half of parking. The path is
    /// not negotiable: `claude --continue` finds its conversation by cwd,
    /// so a different directory is a lost conversation. A path something
    /// else now occupies is refused plainly, never adopted.
    pub fn attach(&self, hr: &HostRef, repo: &str, path: &str, branch: &str) -> Result<()> {
        if hr.exists(path) {
            return Err(anyhow!(
                "{path} already exists — the resume needs its old path back, and \
                 whatever is there now is not this attempt's worktree"
            ));
        }
        if !branch_exists(hr, repo, branch) {
            return Err(anyhow!("{repo} no longer has the branch `{branch}`"));
        }
        // A parked worktree was removed cleanly, but a crash can leave the
        // administrative entry behind; prune so `add` does not refuse over
        // a ghost.
        git(hr, repo, &["worktree", "prune"])?;
        git(hr, repo, &["worktree", "add", path, branch])
            .with_context(|| format!("reattaching a worktree for `{branch}`"))?;
        Ok(())
    }

    /// The diff between two committed states, straight from the object
    /// store — no worktree required. This is how a parked attempt gets its
    /// frozen diff: base against its last checkpoint, which holds tracked
    /// and untracked work alike.
    pub fn diff_range(
        &self,
        hr: &HostRef,
        git_cwd: &str,
        from: &str,
        to: &str,
        dir: &str,
    ) -> Result<String> {
        let prefix = prefix_args(dir);
        git(hr, git_cwd, &with_prefix(&prefix, &[from, to]))
    }

    /// Remove an attempt's root directory once its checkouts have gone.
    ///
    /// Only ever the empty shell a multi-repository attempt leaves behind:
    /// `rmdir` refuses anything still holding a file, which is the guard.
    /// Something left in there is somebody's — a build output, a copied
    /// `.env` — and the directory standing is a smaller surprise than a
    /// tidy-up that took it.
    pub fn remove_root(&self, hr: &HostRef, root: &str) -> Result<()> {
        hr.remove_dir(root)
    }

    /// Give the worktree back.
    ///
    /// The branch is deliberately left alone: it is what a merged attempt was
    /// merged from and what a superseded one can still be looked at through.
    /// Only the working tree goes.
    ///
    /// `--force` because an attempt that is being discarded is exactly the one
    /// with uncommitted work in it, and refusing to clean up in that case
    /// would leave the disk growing forever, which is the failure this step
    /// exists to prevent. The diff is frozen into the attempt row first, so
    /// what is being dropped has already been recorded.
    pub fn remove(&self, hr: &HostRef, repo: &str, path: &str) -> Result<()> {
        if hr.exists(path) {
            git(hr, repo, &["worktree", "remove", "--force", path])
                .with_context(|| format!("removing the worktree at {path}"))?;
        }
        // Clears the administrative entry when the directory was already gone
        // — deleted by hand, or on a volume that did not come back.
        git(hr, repo, &["worktree", "prune"])?;
        Ok(())
    }

    /// Every reason this merge would not go, asked before any of them runs.
    ///
    /// Split out of `merge_to_base` so an attempt spanning several
    /// repositories can be refused as a whole. Discovering the second
    /// repository's uncommitted work after the first has already been merged
    /// leaves a person half-landed, and that is precisely the outcome these
    /// refusals exist to prevent.
    pub fn check_merge(
        &self,
        hr: &HostRef,
        repo: &str,
        worktree: &str,
        branch: &str,
        base_branch: &str,
        locale: Locale,
    ) -> Result<()> {
        let dirty = git(hr, worktree, &["status", "--porcelain"])?;
        if !dirty.trim().is_empty() {
            return Err(anyhow!(i18n::merge_dirty_worktree(locale, branch)));
        }

        let on = git(hr, repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        if on.trim() != base_branch {
            return Err(anyhow!(i18n::merge_wrong_branch(
                locale,
                on.trim(),
                base_branch
            )));
        }
        let repo_dirty = git(hr, repo, &["status", "--porcelain"])?;
        if !repo_dirty.trim().is_empty() {
            return Err(anyhow!(i18n::merge_dirty_base(locale, base_branch)));
        }

        let ahead = git(
            hr,
            repo,
            &["rev-list", "--count", &format!("{base_branch}..{branch}")],
        )?;
        if ahead.trim() == "0" {
            return Err(anyhow!(i18n::merge_nothing_ahead(
                locale,
                branch,
                base_branch
            )));
        }
        Ok(())
    }

    /// Fold an attempt's branch back into the base.
    ///
    /// Every refusal here is one that would otherwise lose work quietly:
    ///
    ///   * The attempt's worktree still has uncommitted changes. Merging the
    ///     branch would produce a merge that does not contain them, and the
    ///     work would sit in a directory that is about to be removed.
    ///   * The main checkout is on some other branch, or has changes of its
    ///     own. Merging into it would rewrite what the person is in the
    ///     middle of.
    ///
    /// Said plainly and refused, rather than worked around: a merge that
    /// silently did something other than what it says is worse than one that
    /// asks you to tidy up first.
    ///
    /// Asked again here even when `check_merge` has just asked: the gap
    /// between the two is a gap a person can commit into, and this is the
    /// question whose answer has to be true at the moment git acts on it.
    pub fn merge_to_base(
        &self,
        hr: &HostRef,
        repo: &str,
        worktree: &str,
        branch: &str,
        base_branch: &str,
        locale: Locale,
    ) -> Result<String> {
        // Asked again, in one call rather than a second copy of the four
        // sentences: the questions have to be true at the moment git acts,
        // but only one place should own how the refusals read.
        self.check_merge(hr, repo, worktree, branch, base_branch, locale)?;

        // `--no-ff` so the attempt stays legible as one piece of work in the
        // history rather than dissolving into the base.
        git(
            hr,
            repo,
            &[
                "merge",
                "--no-ff",
                "-m",
                &format!("Merge {branch} (Marol attempt)"),
                branch,
            ],
        )?;
        git(hr, repo, &["rev-parse", "HEAD"])
    }

    /// Push the attempt's branch and open a pull request for it.
    ///
    /// The push runs from the worktree, which is already on the branch. `gh`
    /// resolves inside the repository's host like everything else, because
    /// its credentials live in that environment, not in ours.
    #[allow(clippy::too_many_arguments)]
    pub fn push_and_open_pr(
        &self,
        hr: &HostRef,
        worktree: &str,
        branch: &str,
        base_branch: &str,
        title: &str,
        body: &str,
        locale: Locale,
    ) -> Result<String> {
        let dirty = git(hr, worktree, &["status", "--porcelain"])?;
        if !dirty.trim().is_empty() {
            return Err(anyhow!(i18n::push_dirty(locale, branch)));
        }

        git(hr, worktree, &["push", "--set-upstream", "origin", branch])?;

        let out = hr
            .run(
                "gh",
                &[
                    "pr", "create", "--base", base_branch, "--head", branch, "--title", title,
                    "--body", body,
                ],
                Some(worktree),
            )
            .map_err(|_| anyhow!(i18n::gh_missing(locale)))?;
        if !out.status.success() {
            return Err(anyhow!(i18n::gh_failed(
                locale,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        // gh prints the URL of the pull request it made.
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// What one of this attempt's checkouts has changed since it started.
    ///
    /// Two calls, because `git diff` only knows about tracked files and an
    /// agent's most common act is creating one. A diff that silently omits
    /// every new file cannot answer "what did this attempt do", which is the
    /// only reason the diff exists.
    ///
    /// `dir` is where this checkout sits inside the attempt, so every path in
    /// the result is relative to the directory the session is standing in —
    /// empty, and the result is the repository-relative diff this has always
    /// produced.
    pub fn diff(&self, hr: &HostRef, worktree: &str, base_sha: &str, dir: &str) -> Result<String> {
        // One crossing, the same fold as `stat` — and for the same reason:
        // this was two git invocations plus one per untracked file, every
        // time the drawer opens on a card with new files in it.
        //
        // The prefixes ride as positional parameters rather than being spliced
        // into the script. They are built from a directory name, and a name is
        // the one thing here nobody controls; `"$@"` hands them to git as argv
        // with no shell reading them on the way.
        let prefix = prefix_args(dir);
        let mut args: Vec<&str> = vec![base_sha];
        args.extend(prefix.iter().map(String::as_str));
        let sections = if matches!(hr.host, Host::Local) {
            let mut untracked = String::new();
            let listed = git(hr, worktree, &["ls-files", "--others", "--exclude-standard"])?;
            for file in listed.lines().filter(|l| !l.trim().is_empty()) {
                let a = with_prefix(&prefix, &["--no-index", "--", "/dev/null", file]);
                if let Ok(o) = hr.run("git", &a, Some(worktree)) {
                    untracked.push_str(&String::from_utf8_lossy(&o.stdout));
                }
            }
            vec![git(hr, worktree, &with_prefix(&prefix, &[base_sha]))?, untracked]
        } else {
            sh_sections(
                hr,
                worktree,
                DIFF_SCRIPT,
                &args,
                2,
                "reading the attempt's diff",
            )?
        };

        // Concatenated exactly as before: the untracked patches follow the
        // tracked ones, and each already ends in its own newline.
        // `trim_end` on the tracked half so the two paths hand back the same
        // string: the local branch goes through `git()`, which trims, and the
        // batched one keeps the newline the script printed before its
        // separator. A diff never opens with whitespace, so nothing is lost.
        let mut out = sections[0].trim_end().to_string();
        let untracked = sections[1].trim_start_matches('\n');
        if !untracked.is_empty() {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(untracked);
        }
        Ok(out)
    }

    /// One file's full text at a committed state — the read-only side of
    /// the editable diff. `None` when the rev does not hold the path: that
    /// is a file the attempt created, not a failure.
    ///
    /// Existence is asked with `ls-tree` (exit status stays clean, output
    /// empty) rather than by matching `git show`'s error prose, which
    /// changes with locale. The content read takes `hr.run` raw — never
    /// the trimming `git()` helper — because file text is content and its
    /// trailing newline is part of it, the same reason `read_to_string`
    /// bypasses `run_ok`.
    pub fn file_at_rev(
        &self,
        hr: &HostRef,
        git_cwd: &str,
        rev: &str,
        path: &str,
    ) -> Result<Option<String>> {
        let listed = git(hr, git_cwd, &["ls-tree", rev, "--", path])?;
        if listed.is_empty() {
            return Ok(None);
        }
        let spec = format!("{rev}:{path}");
        let out = hr
            .run("git", &["show", &spec], Some(git_cwd))
            .with_context(|| format!("running git show {spec}"))?;
        if !out.status.success() {
            return Err(anyhow!(
                "git show {spec} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
    }

    /// The repository's branches, most recently committed first — the
    /// order a person thinks in ("the one I touched yesterday").
    /// Remote-tracking branches count too, stripped of their remote and
    /// deduped, so a base that only exists as `origin/x` is still offered.
    pub fn branches(&self, hr: &HostRef, repo: &str) -> Result<Vec<String>> {
        let out = git(
            hr,
            repo,
            &[
                "for-each-ref",
                "--sort=-committerdate",
                "--format=%(refname)",
                "refs/heads",
                "refs/remotes",
            ],
        )?;
        Ok(parse_branches(&out))
    }

    /// The attempt's footprint at a glance, cheap enough for every card on
    /// the board to ask on a timer: `--numstat` counts rather than the
    /// rendered diff, plus where the branch stands against its base.
    ///
    /// Untracked files go through `--no-index` per file, the same route
    /// `diff` takes — never `add -N`, which would mutate the agent's own
    /// index behind its back to save ourselves a few invocations.
    ///
    /// Ahead and behind are measured against the base *branch* as it is
    /// now, not the recorded base commit: the diff answers "what did this
    /// attempt do", but ahead/behind answers "will the merge go", and the
    /// merge goes against the branch that kept moving.
    pub fn stat(
        &self,
        hr: &HostRef,
        worktree: &str,
        base_sha: &str,
        base_branch: &str,
    ) -> Result<DiffStat> {
        let mut stat = DiffStat::default();

        // One crossing for the whole answer. This used to be four git
        // invocations plus one per untracked file, and the board asks for it
        // every fifteen seconds for every open attempt — locally that is four
        // forks nobody notices, and through `wsl.exe` it is four Windows
        // processes plus one per file, on a timer.
        //
        // The loop moves into the host rather than being removed: `--no-index`
        // against /dev/null is still what renders a new file as the patch that
        // would create it, and `add -N` — the way to avoid it — would mutate
        // the agent's own index behind its back. So the same commands run, in
        // the same order, on the far side of one doorway.
        // Locally there is no doorway to cross, so there is nothing to fold
        // — and there is a positive reason not to: `sh` is not on a Windows
        // login-shell PATH, which is exactly why `Core::list_dir` keeps a
        // native branch of its own. The four native forks cost microseconds
        // here; it is the *crossing* that was expensive.
        let sections = if matches!(hr.host, Host::Local) {
            let range = format!("{base_branch}...HEAD");
            let mut untracked_counts = String::new();
            let listed = git(hr, worktree, &["ls-files", "--others", "--exclude-standard"])?;
            for file in listed.lines().filter(|l| !l.trim().is_empty()) {
                if let Ok(o) = hr.run(
                    "git",
                    &["diff", "--no-index", "--numstat", "--", "/dev/null", file],
                    Some(worktree),
                ) {
                    untracked_counts.push_str(&String::from_utf8_lossy(&o.stdout));
                }
            }
            vec![
                git(hr, worktree, &["diff", "--numstat", base_sha])?,
                untracked_counts,
                git(hr, worktree, &["rev-list", "--left-right", "--count", &range])?,
                git(hr, worktree, &["status", "--porcelain"])?,
            ]
        } else {
            sh_sections(
                hr,
                worktree,
                STAT_SCRIPT,
                &[base_sha, base_branch],
                4,
                "reading the attempt's footprint",
            )?
        };

        for line in sections[0].lines() {
            stat.count(line);
        }
        for line in sections[1].lines() {
            stat.count(line);
        }

        // `left...right` with --left-right --count prints "behind\tahead"
        // from the branch's point of view.
        let mut parts = sections[2].split_whitespace();
        stat.behind = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        stat.ahead = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);

        // Whether anything is still uncommitted — the exact check the merge
        // will make, run ahead of it, so the refusal can become a suggestion
        // before the click instead of an error after it.
        stat.dirty = !sections[3].trim().is_empty();

        Ok(stat)
    }

    /* -------------------------- checkpoints -------------------------- */

    /// The checkpoints an attempt has, oldest first. Read from the refs
    /// themselves, so a restart forgets nothing and the numbering never
    /// restarts into a collision.
    pub fn checkpoints(&self, hr: &HostRef, git_cwd: &str, attempt_id: &str) -> Result<Vec<Checkpoint>> {
        let raw = git(
            hr,
            git_cwd,
            &[
                "for-each-ref",
                "--format=%(refname)\t%(objectname)\t%(creatordate:unix)",
                &checkpoint_prefix(attempt_id),
            ],
        )?;
        let mut out: Vec<Checkpoint> = Vec::new();
        for line in raw.lines() {
            let mut cols = line.split('\t');
            let (Some(refname), Some(sha), Some(at)) = (cols.next(), cols.next(), cols.next())
            else {
                continue;
            };
            let Some(n) = refname.rsplit('/').next().and_then(|s| s.parse::<u64>().ok()) else {
                continue;
            };
            out.push(Checkpoint {
                n,
                sha: sha.to_string(),
                at: at.trim().parse().unwrap_or(0),
            });
        }
        // for-each-ref sorts refnames lexically, where 10 comes before 2.
        out.sort_by_key(|c| c.n);
        Ok(out)
    }

    /// Snapshot the worktree — tracked and untracked alike — touching nothing
    /// the agent sees. A temporary index (`GIT_INDEX_FILE`) takes the `add
    /// -A`, `write-tree` turns it into a tree, `commit-tree` parents it on
    /// the previous checkpoint (or the attempt's base), and a ref under
    /// `refs/marol/checkpoints/<attempt>/<n>` keeps it alive. Worktree,
    /// index, branch, reflog: all exactly as the agent left them — the same
    /// discipline that keeps `stat` away from `add -N`.
    ///
    /// The temp index persists beside the worktree's own git dir, so the
    /// second snapshot onward pays a stat walk, not a re-hash of every file.
    /// A turn that changed nothing produces no ref and returns `None`.
    ///
    /// `n` is handed in rather than counted here, because an attempt spanning
    /// several repositories has to number one moment the same in all of them:
    /// "checkpoint 3" is a moment in the work, not a count of how often one
    /// particular checkout happened to change. A repository untouched at that
    /// moment simply grows no ref for it, and reading back takes the newest
    /// snapshot at or before the number asked for.
    pub fn checkpoint(
        &self,
        hr: &HostRef,
        worktree: &str,
        attempt_id: &str,
        base_sha: &str,
        n: u64,
    ) -> Result<Option<Checkpoint>> {
        let gitdir = git(hr, worktree, &["rev-parse", "--absolute-git-dir"])?;
        let index = format!("{}/marol-checkpoint.index", gitdir.trim_end_matches('/'));
        let snap_env = [("GIT_INDEX_FILE".to_string(), index)];
        hr.run_ok_with_env("git", &["add", "-A"], Some(worktree), &snap_env)?;
        let tree = hr.run_ok_with_env("git", &["write-tree"], Some(worktree), &snap_env)?;

        let existing = self.checkpoints(hr, worktree, attempt_id)?;
        let prev = existing.last();
        let parent = prev.map(|c| c.sha.as_str()).unwrap_or(base_sha);
        let prev_tree = git(hr, worktree, &["rev-parse", &format!("{parent}^{{tree}}")])?;
        if tree == prev_tree {
            return Ok(None);
        }

        // Its own identity, so a repo (or host) with no user.name configured
        // can still snapshot — and no checkpoint ever wears the user's name.
        let id_env = [
            ("GIT_AUTHOR_NAME".to_string(), "Marol".to_string()),
            ("GIT_AUTHOR_EMAIL".to_string(), "checkpoint@marol.local".to_string()),
            ("GIT_COMMITTER_NAME".to_string(), "Marol".to_string()),
            ("GIT_COMMITTER_EMAIL".to_string(), "checkpoint@marol.local".to_string()),
        ];
        let sha = hr.run_ok_with_env(
            "git",
            &["commit-tree", &tree, "-p", parent, "-m", &format!("marol checkpoint {n}")],
            Some(worktree),
            &id_env,
        )?;
        git(
            hr,
            worktree,
            &["update-ref", &format!("{}/{n}", checkpoint_prefix(attempt_id)), &sha],
        )?;
        let at = git(hr, worktree, &["show", "-s", "--format=%ct", &sha])?
            .trim()
            .parse()
            .unwrap_or(0);
        Ok(Some(Checkpoint { n, sha, at }))
    }

    /// Put the worktree's files back to a snapshot — code only. Content is
    /// restored from the snapshot's tree, and files that exist now but not
    /// in it are deleted. The index, the branch, the reflog and the agent's
    /// conversation are never touched: a restore changes exactly what a
    /// person editing files by hand could change, nothing else.
    pub fn restore_checkpoint(&self, hr: &HostRef, worktree: &str, sha: &str) -> Result<()> {
        // What the snapshot holds, against what the worktree holds now —
        // tracked or untracked. Ignored files are neither snapshotted nor
        // deleted: they were never part of the work. NUL separators, so a
        // hostile filename cannot split into two.
        let held = git(hr, worktree, &["ls-tree", "-r", "--name-only", "-z", sha])?;
        let now = git(
            hr,
            worktree,
            &["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        )?;
        let held_set: std::collections::HashSet<&str> =
            held.split('\0').filter(|s| !s.is_empty()).collect();
        for file in now.split('\0').filter(|s| !s.is_empty()) {
            if !held_set.contains(file) {
                hr.remove_file(&hr.join(worktree, file))?;
            }
        }
        // Everything the snapshot does hold comes back as it was. Worktree
        // only — `--staged` is exactly the flag this must never grow.
        git(hr, worktree, &["restore", "--source", sha, "--worktree", "--", ":/"])?;
        Ok(())
    }

    /// Delete every checkpoint ref an attempt holds. Run at the attempt's
    /// end: from then on the frozen diff is the one record, and the refs
    /// would otherwise pin every snapshot's objects forever. `git_cwd` is
    /// any directory of the repository — the refs live in the shared git
    /// dir, so the main checkout works after the worktree is gone.
    pub fn clear_checkpoints(&self, hr: &HostRef, git_cwd: &str, attempt_id: &str) -> Result<()> {
        let raw = git(
            hr,
            git_cwd,
            &["for-each-ref", "--format=%(refname)", &checkpoint_prefix(attempt_id)],
        )?;
        for r in raw.lines().map(str::trim).filter(|r| !r.is_empty()) {
            git(hr, git_cwd, &["update-ref", "-d", r])?;
        }
        Ok(())
    }

    /// Drop checkpoint refs whose attempt is no longer open — the crash
    /// leftovers. Run once per repository at startup; returns how many refs
    /// went.
    pub fn sweep_checkpoints(
        &self,
        hr: &HostRef,
        repo: &str,
        live_attempts: &std::collections::HashSet<String>,
    ) -> Result<usize> {
        let raw = git(
            hr,
            repo,
            &["for-each-ref", "--format=%(refname)", "refs/marol/checkpoints"],
        )?;
        let mut swept = 0;
        for r in raw.lines().map(str::trim).filter(|r| !r.is_empty()) {
            let attempt = r
                .strip_prefix("refs/marol/checkpoints/")
                .and_then(|rest| rest.split('/').next());
            if let Some(id) = attempt {
                if !live_attempts.contains(id) {
                    git(hr, repo, &["update-ref", "-d", r])?;
                    swept += 1;
                }
            }
        }
        Ok(swept)
    }
}

/// One numbered snapshot of an attempt's worktree, held by a private ref.
#[derive(Debug, Clone, Serialize)]
pub struct Checkpoint {
    /// Ordinal within the attempt, starting at 1 — `base_sha` is the free
    /// zeroth.
    pub n: u64,
    /// The snapshot commit. Diffable and restorable like any tree-ish.
    pub sha: String,
    /// Unix seconds, from the snapshot commit's own clock.
    pub at: i64,
}

fn checkpoint_prefix(attempt_id: &str) -> String {
    format!("refs/marol/checkpoints/{attempt_id}")
}

/// The snapshot a checkout actually holds for the moment numbered `n`.
///
/// The newest one at or before it, because a repository that was untouched
/// at that moment grew no ref for it, and the honest answer to "how did this
/// checkout look at checkpoint 3" is then "the way it looked at 2". `None`
/// means it had not changed at all yet, and the attempt's base is the answer.
pub fn at_or_before(checkpoints: &[Checkpoint], n: u64) -> Option<&Checkpoint> {
    checkpoints.iter().rfind(|c| c.n <= n)
}

/// Full refnames into offerable branch names: `refs/heads/x` and
/// `refs/remotes/<remote>/x` both become `x`, first (most recent) sighting
/// wins, remote HEAD pointers are noise and dropped. Full refnames rather
/// than shorthand, because a local `feature/x` and a remote `origin/x` are
/// indistinguishable once shortened. Capped: past fifty a picker is a
/// search box, and typing already filters.
fn parse_branches(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in raw.lines() {
        let name = if let Some(rest) = line.trim().strip_prefix("refs/heads/") {
            rest
        } else if let Some(rest) = line.trim().strip_prefix("refs/remotes/") {
            match rest.split_once('/') {
                Some((_, branch)) => branch,
                None => continue,
            }
        } else {
            continue;
        };
        if name.is_empty() || name == "HEAD" || out.iter().any(|b| b == name) {
            continue;
        }
        out.push(name.to_string());
        if out.len() >= 50 {
            break;
        }
    }
    out
}

/// What `stat` measures. Serialized as-is to the UI.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DiffStat {
    /// Files touched, counting untracked ones — an agent's commonest act.
    pub files: i64,
    pub adds: i64,
    pub dels: i64,
    /// Commits the branch has that the base does not.
    pub ahead: i64,
    /// Commits the base has grown since — the merge refusal not yet hit.
    pub behind: i64,
    /// Uncommitted work in the worktree — the other refusal not yet hit.
    pub dirty: bool,
}

impl DiffStat {
    /// One `--numstat` line: `adds\tdels\tpath`. A binary file prints `-`
    /// for both counts; it is still a touched file, just not counted lines.
    fn count(&mut self, line: &str) {
        let mut cols = line.split('\t');
        let (Some(a), Some(d)) = (cols.next(), cols.next()) else {
            return;
        };
        if cols.next().is_none() {
            return;
        }
        self.files += 1;
        self.adds += a.parse::<i64>().unwrap_or(0);
        self.dels += d.parse::<i64>().unwrap_or(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /* -------------------------- branches ---------------------------- */

    #[test]
    fn refnames_become_offerable_branches_in_order() {
        let raw = "refs/heads/fix-login\nrefs/remotes/origin/main\nrefs/heads/feature/x\nrefs/remotes/origin/HEAD\nrefs/remotes/origin/fix-login\nrefs/remotes/upstream/main\n";
        assert_eq!(
            parse_branches(raw),
            vec!["fix-login", "main", "feature/x"]
        );
    }

    /// A local `feature/x` must survive shortening — the reason the parse
    /// works on full refnames rather than `refname:short`.
    #[test]
    fn a_local_branch_with_a_slash_is_not_mistaken_for_a_remote() {
        assert_eq!(parse_branches("refs/heads/feature/login\n"), vec!["feature/login"]);
    }

    #[test]
    fn the_list_is_capped_at_fifty() {
        let raw: String = (0..80).map(|i| format!("refs/heads/b{i}\n")).collect();
        assert_eq!(parse_branches(&raw).len(), 50);
    }

    /* -------------------------- diffstat ---------------------------- */

    #[test]
    fn numstat_lines_are_counted() {
        let mut s = DiffStat::default();
        s.count("12\t3\tsrc/app.ts");
        s.count("0\t7\tREADME.md");
        // A binary file prints `-` for both counts; it is still a file
        // the attempt touched, just not countable lines.
        s.count("-\t-\tlogo.png");
        assert_eq!(
            s,
            DiffStat { files: 3, adds: 12, dels: 10, ..DiffStat::default() }
        );
    }

    /// Whatever is not a numstat row — blank lines, stray warnings on
    /// stdout — must not count as a touched file.
    #[test]
    fn noise_is_not_a_file() {
        let mut s = DiffStat::default();
        s.count("");
        s.count("warning: exhaustive rename detection was skipped");
        s.count("12\t3");
        assert_eq!(s, DiffStat::default());
    }

    /* --------------------------- slugs ----------------------------- */

    #[test]
    fn a_title_becomes_something_git_will_accept() {
        assert_eq!(slug("Fix the login bug", "abc123"), "fix-the-login-bug");
        assert_eq!(slug("Add /api/v2 endpoint", "abc123"), "add-api-v2-endpoint");
    }

    /// Titles here are usually Chinese. A mixed one keeps whatever ASCII it
    /// has, which is normally the part a person would grep for anyway.
    #[test]
    fn a_mixed_title_keeps_its_ascii() {
        assert_eq!(slug("修好登入 bug", "abc123"), "bug");
    }

    /// The one that would produce `marol/-1` and a directory called `-1`.
    #[test]
    fn a_title_with_no_ascii_falls_back_to_the_task_id() {
        assert_eq!(slug("修好登入頁面", "9f8e7d6c-1111"), "task-9f8e7d6c");
        assert_eq!(slug("！！！", "abcdef12"), "task-abcdef12");
        assert_eq!(slug("", "abcdef12"), "task-abcdef12");
    }

    /// Leading and trailing punctuation must not survive as dashes: git
    /// rejects a branch component that starts with one, and a trailing dash
    /// makes `<slug>-<n>` read as `--`.
    #[test]
    fn punctuation_never_becomes_a_leading_or_trailing_dash() {
        for title in ["  spaces  ", "--dashes--", "...dots...", "(parens)"] {
            let s = slug(title, "abc123");
            assert!(!s.starts_with('-'), "{title:?} produced {s:?}");
            assert!(!s.ends_with('-'), "{title:?} produced {s:?}");
            assert!(!s.contains("--"), "{title:?} produced {s:?}");
        }
    }

    #[test]
    fn a_long_title_is_cut_to_a_readable_length() {
        let s = slug(&"word ".repeat(50), "abc123");
        assert!(s.len() <= MAX_SLUG, "{s:?} is {} chars", s.len());
        assert!(!s.ends_with('-'));
    }

    /* --------------------------- layout ---------------------------- */

    /// Two repositories can easily share a folder name. If their worktrees
    /// shared a directory, attempt `api-1` of one would collide with `api-1`
    /// of the other.
    #[test]
    fn repositories_with_the_same_name_do_not_share_a_directory() {
        let a = dir_for(&Host::Local, "/tmp/root", "/Users/x/work/api");
        let b = dir_for(&Host::Local, "/tmp/root", "/Users/x/side/api");
        assert_ne!(a, b);
        assert!(a.contains("api-"));
        assert!(b.contains("api-"));
    }

    #[test]
    fn the_same_repository_always_gets_the_same_directory() {
        assert_eq!(
            dir_for(&Host::Local, "/tmp/root", "/Users/x/work/api"),
            dir_for(&Host::Local, "/tmp/root", "/Users/x/work/api")
        );
    }

    /// A distro-side layout is POSIX regardless of what the app runs on.
    #[test]
    fn a_wsl_repositorys_worktrees_land_under_its_own_root() {
        let host = Host::Wsl {
            distro: "Ubuntu".into(),
        };
        let dir = dir_for(&host, "/home/me/.marol/worktrees", "/home/me/code/api");
        assert!(
            dir.starts_with("/home/me/.marol/worktrees/api-"),
            "{dir}"
        );
        assert!(!dir.contains('\\'));
    }

    /// Worktrees must not land inside a repository, because a repository's
    /// parent is so often another repository.
    #[test]
    fn the_default_root_is_not_beside_the_repository() {
        let root = Worktrees::default_root();
        assert!(root.ends_with("worktrees"));
        let s = root.to_string_lossy().to_string();
        assert!(s.contains(".marol") || s.contains(".agentdesk"), "{s}");
    }

    /// A desk that already has worktrees keeps them where they are.
    ///
    /// Everything else the rename touched was ours alone to move. These are
    /// not: each path is written into the attempt row that opened it, and
    /// registered inside its repository's `.git/worktrees/<id>/gitdir` with
    /// the tree's own `.git` file pointing back. Moving the directory breaks
    /// both ends and leaves every open attempt pointing at nothing — a rename
    /// that reaches into somebody's repositories to repair itself has stopped
    /// being a rename.
    #[test]
    fn worktrees_from_before_the_rename_are_left_exactly_where_they_are() {
        let home = std::env::temp_dir().join(format!("marol-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);

        // A fresh machine has neither, and gets the new name.
        std::fs::create_dir_all(&home).unwrap();
        assert_eq!(
            Worktrees::default_root_in(&home),
            home.join(".marol").join("worktrees")
        );

        // One with trees from before goes on using them, under the old name,
        // for as long as they are there.
        std::fs::create_dir_all(home.join(".agentdesk").join("worktrees")).unwrap();
        assert_eq!(
            Worktrees::default_root_in(&home),
            home.join(".agentdesk").join("worktrees"),
        );

        // And moves on by itself once the last one is handed back.
        std::fs::remove_dir_all(home.join(".agentdesk")).unwrap();
        assert_eq!(
            Worktrees::default_root_in(&home),
            home.join(".marol").join("worktrees")
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
