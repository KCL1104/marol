//! Worktree isolation, against real git.
//!
//! The properties here are the ones that decide whether running two agents on
//! one repository is real or only looks real: that the two never see each
//! other's files, that each diffs against the commit it actually started
//! from, and that the disk is given back afterwards.
//!
//!     cargo test --test worktree -- --nocapture

use std::path::{Path, PathBuf};

#[path = "../src/channel.rs"]
mod channel;
#[path = "../src/host.rs"]
mod host;
#[path = "../src/shell_env.rs"]
mod shell_env;
#[path = "../src/i18n.rs"]
mod i18n;
#[path = "../src/worktree.rs"]
mod worktree;

use crate::host::{Host, HostRef};
use crate::shell_env::ShellEnv;
use crate::worktree::{slug, OpenedTree, OpenedWorktree, RepoSpec, Worktrees};

fn spec(repo: &str, base_branch: &str) -> RepoSpec {
    RepoSpec {
        repo: repo.to_string(),
        base_branch: base_branch.to_string(),
    }
}

fn env() -> ShellEnv {
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(shell_env::resolve())
}

/// These tests exercise the git layer itself, on the machine they run on.
static LOCAL: Host = Host::Local;

fn hr(env: &ShellEnv) -> HostRef<'_> {
    HostRef {
        host: &LOCAL,
        local: env,
        env,
        // Local, so there is no doorway and nothing to hold open.
        channels: None,
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must be installed");
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with one commit on `main`, plus a worktree root beside it.
struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    trees: Worktrees,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "marol-wt-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        git(&repo, &["init", "-b", "main", "-q"]);
        git(&repo, &["config", "user.email", "t@marol.test"]);
        git(&repo, &["config", "user.name", "Marol Test"]);
        std::fs::write(repo.join("app.txt"), "one\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "first"]);

        let trees = Worktrees::new(root.join("worktrees"));
        Self { root, repo, trees }
    }

    fn repo_s(&self) -> String {
        self.repo.to_string_lossy().to_string()
    }

    fn head(&self) -> String {
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    fn commit_on_main(&self, contents: &str) {
        std::fs::write(self.repo.join("app.txt"), contents).unwrap();
        git(&self.repo, &["add", "-A"]);
        git(&self.repo, &["commit", "-qm", "another"]);
    }

    /// A second repository beside the first, for the cards that span two.
    /// `where_` is its directory name under the fixture root, so a test can
    /// ask for one that collides with the first's.
    fn second_repo(&self, where_: &str) -> String {
        let repo = self.root.join("other").join(where_);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main", "-q"]);
        git(&repo, &["config", "user.email", "t@marol.test"]);
        git(&repo, &["config", "user.name", "Marol Test"]);
        std::fs::write(repo.join("app.txt"), "one\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "first"]);
        repo.to_string_lossy().to_string()
    }

    /// One attempt on this fixture's only repository.
    fn attempt(&self, env: &ShellEnv, slug: &str, seq: i64) -> OpenedWorktree {
        self.trees
            .create(
                &hr(env),
                &self.trees.local_root(),
                &[spec(&self.repo_s(), "main")],
                slug,
                seq,
            )
            .expect("opening an attempt")
    }

    /// The same thing, as the one checkout it is. A card naming one
    /// repository puts the checkout *at* the attempt's root, so the tree is
    /// the whole of what these tests are looking at.
    fn open(&self, env: &ShellEnv, slug: &str, seq: i64) -> OpenedTree {
        let mut wt = self.attempt(env, slug, seq);
        assert_eq!(wt.root, wt.trees[0].path, "one repository, one directory");
        wt.trees.remove(0)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The point of the whole layer: two agents on one repository, at once,
/// without either seeing what the other is doing.
#[test]
fn two_attempts_on_one_repository_do_not_see_each_other() {
    let env = env();
    let f = Fixture::new("isolation");

    let a = f.open(&env, "login", 1);
    let b = f.open(&env, "login", 2);

    assert_ne!(a.path, b.path);
    assert_eq!(a.branch, "marol/login-1");
    assert_eq!(b.branch, "marol/login-2");

    // Each agent writes in its own tree.
    std::fs::write(Path::new(&a.path).join("app.txt"), "from attempt one\n").unwrap();
    std::fs::write(Path::new(&a.path).join("only-in-a.txt"), "a\n").unwrap();
    std::fs::write(Path::new(&b.path).join("app.txt"), "from attempt two\n").unwrap();

    assert_eq!(
        std::fs::read_to_string(Path::new(&b.path).join("app.txt")).unwrap(),
        "from attempt two\n",
        "one attempt's edit reached the other's tree"
    );
    assert!(
        !Path::new(&b.path).join("only-in-a.txt").exists(),
        "a file created in one attempt appeared in the other"
    );
    // And the repository the person actually works in is untouched.
    assert_eq!(
        std::fs::read_to_string(f.repo.join("app.txt")).unwrap(),
        "one\n",
        "an attempt wrote into the main checkout"
    );
}

/// `main` keeps moving. An attempt has to diff against the commit it started
/// from, or its diff picks up everything that landed on the base afterwards
/// and stops being a description of what the agent did.
#[test]
fn each_attempt_records_the_base_it_actually_started_from() {
    let env = env();
    let f = Fixture::new("basesha");

    let first_base = f.head();
    let a = f.open(&env, "card", 1);
    assert_eq!(a.base_sha, first_base);

    f.commit_on_main("two\n");
    let second_base = f.head();
    assert_ne!(first_base, second_base);

    let b = f.open(&env, "card", 2);
    assert_eq!(b.base_sha, second_base);
    // The first attempt's baseline did not move under it.
    assert_eq!(a.base_sha, first_base);
}

/// Skipping cleanup is how the disk fills up, so the removal has to work even
/// in the case that provokes it: an attempt abandoned with work still in the
/// tree.
#[test]
fn removing_a_worktree_gives_the_disk_back_and_keeps_the_branch() {
    let env = env();
    let f = Fixture::new("cleanup");

    let a = f.open(&env, "card", 1);
    std::fs::write(Path::new(&a.path).join("app.txt"), "uncommitted\n").unwrap();
    std::fs::write(Path::new(&a.path).join("scratch.txt"), "junk\n").unwrap();
    assert!(Path::new(&a.path).exists());

    f.trees.remove(&hr(&env), &f.repo_s(), &a.path).expect("remove");

    assert!(!Path::new(&a.path).exists(), "the worktree directory is still on disk");
    let listed = git(&f.repo, &["worktree", "list"]);
    assert!(
        !listed.contains(&a.path),
        "git still lists the worktree: {listed}"
    );
    // The branch is what a merged attempt was merged from; it stays.
    assert!(
        git(&f.repo, &["branch", "--list", "marol/card-1"]).contains("marol/card-1"),
        "removing the worktree took the branch with it"
    );
}

#[test]
fn removing_a_worktree_whose_directory_is_already_gone_still_tidies_up() {
    let env = env();
    let f = Fixture::new("gonedir");

    let a = f.open(&env, "card", 1);
    // Deleted by hand, or an external volume that did not come back.
    std::fs::remove_dir_all(&a.path).unwrap();

    f.trees
        .remove(&hr(&env), &f.repo_s(), &a.path)
        .expect("a missing directory must not make cleanup fail");
    let listed = git(&f.repo, &["worktree", "list"]);
    assert!(!listed.contains("card-1"), "stale entry left behind: {listed}");
}

/// A branch outlives the row that recorded it. Delete a card, make another
/// with the same title, and numbering starts over onto branches git still
/// has — so the numbering has to walk past them rather than fail.
#[test]
fn a_branch_git_already_has_is_walked_past() {
    let env = env();
    let f = Fixture::new("collision");

    git(&f.repo, &["branch", "marol/card-1"]);
    git(&f.repo, &["branch", "marol/card-2"]);

    let a = f.attempt(&env, "card", 1);
    assert_eq!(a.branch, "marol/card-3");
    assert_eq!(a.seq, 3, "the number actually taken has to be reported back");
}

/// End to end for the case the slug rules exist for: git has to accept the
/// branch name a Chinese title produces.
#[test]
fn a_title_with_no_ascii_still_produces_a_branch_git_accepts() {
    let env = env();
    let f = Fixture::new("cjk");

    let s = slug("修好登入頁面的白畫面", "9f8e7d6c-4b2a");
    let a = f.open(&env, &s, 1);

    assert_eq!(a.branch, "marol/task-9f8e7d6c-1");
    assert!(Path::new(&a.path).exists());
    assert_eq!(
        git(Path::new(&a.path), &["rev-parse", "--abbrev-ref", "HEAD"]),
        "marol/task-9f8e7d6c-1"
    );
}

/// `git diff` alone knows nothing about files that were never added, and
/// creating files is most of what an agent does. A diff that omits them
/// cannot answer the question the diff tab exists to answer.
#[test]
fn the_diff_shows_files_the_agent_created_as_well_as_ones_it_edited() {
    let env = env();
    let f = Fixture::new("diff");

    let a = f.open(&env, "card", 1);
    std::fs::write(Path::new(&a.path).join("app.txt"), "edited by the agent\n").unwrap();
    std::fs::write(Path::new(&a.path).join("brand_new.rs"), "fn main() {}\n").unwrap();

    let diff = f.trees.diff(&hr(&env), &a.path, &a.base_sha, "").expect("diff");

    assert!(diff.contains("edited by the agent"), "the edit is missing:\n{diff}");
    assert!(
        diff.contains("brand_new.rs"),
        "a file the agent created is missing from the diff:\n{diff}"
    );
    assert!(
        diff.contains("fn main() {}"),
        "the new file's contents are missing:\n{diff}"
    );
}

/// Committed work has to stay in the diff too — the prompt asks the agent to
/// commit, and the diff is against the base, not against HEAD.
#[test]
fn the_diff_covers_committed_and_uncommitted_work_together() {
    let env = env();
    let f = Fixture::new("committed");

    let a = f.open(&env, "card", 1);
    std::fs::write(Path::new(&a.path).join("done.txt"), "committed work\n").unwrap();
    git(Path::new(&a.path), &["add", "-A"]);
    git(Path::new(&a.path), &["commit", "-qm", "agent's commit"]);
    std::fs::write(Path::new(&a.path).join("app.txt"), "still in progress\n").unwrap();

    let diff = f.trees.diff(&hr(&env), &a.path, &a.base_sha, "").unwrap();
    assert!(diff.contains("committed work"), "committed work missing:\n{diff}");
    assert!(diff.contains("still in progress"), "uncommitted work missing:\n{diff}");
}

/* --------------------------- several repos --------------------------- */

/// The shape of a card that spans two repositories: one workspace, one
/// directory per repository inside it, one branch name in both — and the
/// person's own checkouts untouched, which is the safety argument surviving
/// the generalisation.
#[test]
fn a_card_spanning_two_repositories_opens_a_worktree_in_each() {
    let env = env();
    let f = Fixture::new("multi");
    let other = f.second_repo("api");

    let wt = f
        .trees
        .create(
            &hr(&env),
            &f.trees.local_root(),
            &[spec(&f.repo_s(), "main"), spec(&other, "main")],
            "login",
            1,
        )
        .expect("two repositories, one attempt");

    assert_eq!(wt.trees.len(), 2);
    assert_eq!(wt.branch, "marol/login-1");
    // The workspace is not itself a checkout; the checkouts are under it.
    assert_eq!(wt.trees[0].dir, "repo");
    assert_eq!(wt.trees[1].dir, "api");
    for tree in &wt.trees {
        assert_eq!(
            tree.path,
            Path::new(&wt.root).join(&tree.dir).to_string_lossy(),
            "a checkout must sit inside the attempt's own directory"
        );
        assert_eq!(
            git(Path::new(&tree.path), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "marol/login-1",
            "both checkouts are one piece of work under one branch name"
        );
    }
    assert!(!Path::new(&wt.root).join(".git").exists(), "the workspace is not a repo");

    // Neither person-facing checkout moved.
    for repo in [f.repo_s(), other] {
        assert_eq!(
            git(Path::new(&repo), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "main"
        );
    }
}

/// The numbering is one answer for the whole attempt. A branch name free in
/// one repository but taken in the other must not be handed out, or
/// `marol/card-2` would mean two different things inside one workspace.
#[test]
fn the_attempt_number_walks_past_a_branch_any_of_the_repositories_has() {
    let env = env();
    let f = Fixture::new("multi-seq");
    let other = f.second_repo("api");
    // Only the second repository has it — the first would have said yes.
    git(Path::new(&other), &["branch", "marol/card-1"]);

    let wt = f
        .trees
        .create(
            &hr(&env),
            &f.trees.local_root(),
            &[spec(&f.repo_s(), "main"), spec(&other, "main")],
            "card",
            1,
        )
        .unwrap();
    assert_eq!(wt.branch, "marol/card-2");
    assert_eq!(wt.seq, 2);
}

/// The diff is one diff, and its paths are relative to where the session is
/// standing — which is what lets a review comment name `api/routes.py` and
/// have the agent find it.
#[test]
fn each_checkouts_diff_paths_are_relative_to_the_workspace() {
    let env = env();
    let f = Fixture::new("multi-diff");
    let other = f.second_repo("api");

    let wt = f
        .trees
        .create(
            &hr(&env),
            &f.trees.local_root(),
            &[spec(&f.repo_s(), "main"), spec(&other, "main")],
            "card",
            1,
        )
        .unwrap();

    std::fs::write(Path::new(&wt.trees[0].path).join("app.txt"), "edited\n").unwrap();
    std::fs::write(Path::new(&wt.trees[1].path).join("brand_new.rs"), "fn main() {}\n").unwrap();

    let first = f
        .trees
        .diff(&hr(&env), &wt.trees[0].path, &wt.trees[0].base_sha, &wt.trees[0].dir)
        .unwrap();
    assert!(first.contains("a/repo/app.txt"), "not workspace-relative:\n{first}");
    assert!(first.contains("b/repo/app.txt"), "not workspace-relative:\n{first}");

    // A file the agent created goes through `--no-index`, and has to wear
    // the same prefix — otherwise half a diff points somewhere else.
    let second = f
        .trees
        .diff(&hr(&env), &wt.trees[1].path, &wt.trees[1].base_sha, &wt.trees[1].dir)
        .unwrap();
    assert!(
        second.contains("b/api/brand_new.rs"),
        "a created file lost its checkout prefix:\n{second}"
    );
}

/// Two repositories can easily share a name, and a workspace with two `api/`
/// in it could not exist. The second takes the same path hash the worktree
/// directories are keyed by.
#[test]
fn two_repositories_with_one_name_still_get_two_directories() {
    let env = env();
    let f = Fixture::new("multi-name");
    let twin = f.second_repo("repo");

    let wt = f
        .trees
        .create(
            &hr(&env),
            &f.trees.local_root(),
            &[spec(&f.repo_s(), "main"), spec(&twin, "main")],
            "card",
            1,
        )
        .unwrap();
    assert_eq!(wt.trees[0].dir, "repo");
    assert_ne!(wt.trees[1].dir, "repo");
    assert!(wt.trees[1].dir.starts_with("repo-"), "{}", wt.trees[1].dir);
    assert!(Path::new(&wt.trees[0].path).join("app.txt").exists());
    assert!(Path::new(&wt.trees[1].path).join("app.txt").exists());
}

/// A workspace half opened is worse than none: it would diff as if the
/// missing repository had no changes in it. So a failure takes back what it
/// already made.
#[test]
fn a_failure_part_way_through_leaves_no_half_made_workspace() {
    let env = env();
    let f = Fixture::new("multi-unwind");
    let other = f.second_repo("api");
    // The second repository has no `develop`, so its worktree cannot open —
    // but only after the first one's has.
    let err = f
        .trees
        .create(
            &hr(&env),
            &f.trees.local_root(),
            &[spec(&f.repo_s(), "main"), spec(&other, "develop")],
            "card",
            1,
        )
        .expect_err("a missing base branch must refuse the whole attempt");
    assert!(err.to_string().contains("no branch `develop`"), "{err}");

    // Nothing of the first repository's survived it, in git or on disk.
    assert_eq!(
        git(&f.repo, &["branch", "--list", "marol/card-1"]).trim(),
        "",
        "the first repository kept a branch for an attempt that never opened"
    );
    let listed = git(&f.repo, &["worktree", "list"]);
    assert!(!listed.contains("card-1"), "a worktree outlived the failure: {listed}");
}

/* --------------------------- file at rev ---------------------------- */

/// The base side of the editable diff: the file as the base commit holds
/// it, byte for byte — the trailing newline included, because the merge
/// view diffs this text against the worktree's and a shaved newline would
/// invent a change on every file.
#[test]
fn a_files_text_at_the_base_comes_back_exactly() {
    let env = env();
    let f = Fixture::new("file-at-rev");
    let a = f.open(&env, "card", 1);
    std::fs::write(Path::new(&a.path).join("app.txt"), "changed since\n").unwrap();

    let base = f
        .trees
        .file_at_rev(&hr(&env), &a.path, &a.base_sha, "app.txt")
        .unwrap();
    assert_eq!(base.as_deref(), Some("one\n"), "must be the base copy, untrimmed");
}

/// A file the attempt created has no base side — that is `None`, not an
/// error, because "new file" is the commonest thing an agent does.
#[test]
fn a_file_the_attempt_created_has_no_base_side() {
    let env = env();
    let f = Fixture::new("file-new");
    let a = f.open(&env, "card", 1);
    std::fs::write(Path::new(&a.path).join("fresh.rs"), "fn main() {}\n").unwrap();

    let base = f
        .trees
        .file_at_rev(&hr(&env), &a.path, &a.base_sha, "fresh.rs")
        .unwrap();
    assert!(base.is_none());

    // A rev git has never heard of is a real failure, not a quiet None —
    // that would dress corrupt state up as "new file".
    assert!(f
        .trees
        .file_at_rev(&hr(&env), &a.path, "0000000000000000000000000000000000000000", "app.txt")
        .is_err());
}

/* ---------------------------- preconditions ---------------------------- */

#[test]
fn a_directory_that_is_not_a_repository_is_refused_up_front() {
    let env = env();
    let dir = std::env::temp_dir().join(format!("marol-notrepo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trees = Worktrees::new(dir.join("worktrees"));

    let err = trees
        .check_repo(&hr(&env), &dir.to_string_lossy(), "main")
        .expect_err("a plain directory must not be accepted as a repository");
    assert!(
        err.to_string().contains("not a git repository"),
        "unhelpful error: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_base_branch_that_does_not_exist_is_refused_up_front() {
    let env = env();
    let f = Fixture::new("nobranch");

    let err = f
        .trees
        .check_repo(&hr(&env), &f.repo_s(), "develop")
        .expect_err("a missing base branch must be caught when the card is made");
    assert!(
        err.to_string().contains("no branch `develop`"),
        "unhelpful error: {err}"
    );
    // And nothing was created on the way to finding out.
    assert!(!Path::new(&f.trees.local_root()).exists());
}

/* --------------------------- checkpoints ---------------------------- */

/// The philosophy acceptance from the decision document: a checkpoint
/// produces a ref, and the agent's own `git status` — worktree, index,
/// branch — reads exactly the same before and after.
#[test]
fn a_checkpoint_leaves_a_ref_and_the_agents_git_status_untouched() {
    let env = env();
    let f = Fixture::new("ckpt-status");
    let a = f.open(&env, "card", 1);

    // The agent's typical mess: a tracked edit, a new file, a staged file.
    std::fs::write(Path::new(&a.path).join("app.txt"), "edited\n").unwrap();
    std::fs::write(Path::new(&a.path).join("fresh.txt"), "new\n").unwrap();
    std::fs::write(Path::new(&a.path).join("staged.txt"), "staged\n").unwrap();
    git(Path::new(&a.path), &["add", "staged.txt"]);

    let before = git(Path::new(&a.path), &["status", "--porcelain=v2", "--branch"]);
    let cp = f
        .trees
        .checkpoint(&hr(&env), &a.path, "attempt-1", &a.base_sha, 1)
        .unwrap()
        .expect("real changes must produce a checkpoint");
    let after = git(Path::new(&a.path), &["status", "--porcelain=v2", "--branch"]);

    assert_eq!(before, after, "the snapshot moved the agent's git state");
    assert_eq!(cp.n, 1);

    // The snapshot holds everything, untracked and staged alike.
    let held = git(f.repo.as_path(), &["ls-tree", "-r", "--name-only", &cp.sha]);
    for name in ["app.txt", "fresh.txt", "staged.txt"] {
        assert!(held.lines().any(|l| l == name), "{name} missing from the snapshot");
    }
    let refs = git(f.repo.as_path(), &["for-each-ref", "refs/marol/checkpoints"]);
    assert!(refs.contains("refs/marol/checkpoints/attempt-1/1"));
}

/// A quiet turn adds nothing: same tree, no new ref — whatever number it was
/// offered. The number itself comes from the core, which shares one across an
/// attempt's checkouts; what this layer owes is that a moment with nothing in
/// it leaves no trace.
#[test]
fn an_unchanged_worktree_produces_no_new_checkpoint() {
    let env = env();
    let f = Fixture::new("ckpt-quiet");
    let a = f.open(&env, "card", 1);

    // Nothing has changed since base: even the first ask is a no-op.
    assert!(f
        .trees
        .checkpoint(&hr(&env), &a.path, "attempt-1", &a.base_sha, 1)
        .unwrap()
        .is_none());

    std::fs::write(Path::new(&a.path).join("app.txt"), "round one\n").unwrap();
    let one = f
        .trees
        .checkpoint(&hr(&env), &a.path, "attempt-1", &a.base_sha, 1)
        .unwrap()
        .unwrap();
    assert_eq!(one.n, 1);
    assert!(f
        .trees
        .checkpoint(&hr(&env), &a.path, "attempt-1", &a.base_sha, 2)
        .unwrap()
        .is_none());

    std::fs::write(Path::new(&a.path).join("app.txt"), "round two\n").unwrap();
    let two = f
        .trees
        .checkpoint(&hr(&env), &a.path, "attempt-1", &a.base_sha, 2)
        .unwrap()
        .unwrap();
    assert_eq!(two.n, 2, "the number asked for is the number written down");

    let list = f.trees.checkpoints(&hr(&env), &a.path, "attempt-1").unwrap();
    assert_eq!(list.iter().map(|c| c.n).collect::<Vec<_>>(), vec![1, 2]);
    // Each snapshot parents on the one before, so "what did this turn do"
    // is one diff away.
    let parent = git(f.repo.as_path(), &["rev-parse", &format!("{}^", two.sha)]);
    assert_eq!(parent, one.sha);
}

/// The end of an attempt takes its refs with it; the sweep catches what a
/// crash left behind — and only that.
#[test]
fn refs_are_cleared_at_the_end_and_orphans_are_swept() {
    let env = env();
    let f = Fixture::new("ckpt-clear");
    let a = f.open(&env, "card", 1);

    std::fs::write(Path::new(&a.path).join("app.txt"), "live\n").unwrap();
    f.trees
        .checkpoint(&hr(&env), &a.path, "attempt-live", &a.base_sha, 1)
        .unwrap()
        .unwrap();
    std::fs::write(Path::new(&a.path).join("app.txt"), "dead\n").unwrap();
    f.trees
        .checkpoint(&hr(&env), &a.path, "attempt-dead", &a.base_sha, 1)
        .unwrap()
        .unwrap();

    // The finished attempt's refs go, from the main checkout — the worktree
    // may already be gone by then.
    f.trees
        .clear_checkpoints(&hr(&env), &f.repo_s(), "attempt-dead")
        .unwrap();
    let refs = git(f.repo.as_path(), &["for-each-ref", "refs/marol/checkpoints"]);
    assert!(!refs.contains("attempt-dead"));
    assert!(refs.contains("attempt-live"));

    // The sweep with only `attempt-live` open leaves it alone and reports
    // nothing to do; with nothing open it takes the leftovers.
    let live: std::collections::HashSet<String> =
        std::iter::once("attempt-live".to_string()).collect();
    assert_eq!(f.trees.sweep_checkpoints(&hr(&env), &f.repo_s(), &live).unwrap(), 0);
    let none: std::collections::HashSet<String> = Default::default();
    assert_eq!(f.trees.sweep_checkpoints(&hr(&env), &f.repo_s(), &none).unwrap(), 1);
    let refs = git(f.repo.as_path(), &["for-each-ref", "refs/marol/checkpoints"]);
    assert_eq!(refs.trim(), "");
}

/// Restore is code only: the worktree comes back to the snapshot exactly —
/// contents, resurrected deletions, extra files gone — while the index keeps
/// whatever the agent had staged, because the conversation-side state is
/// never ours to touch.
#[test]
fn restore_returns_the_worktree_to_the_snapshot_and_only_the_worktree() {
    let env = env();
    let f = Fixture::new("ckpt-restore");
    let a = f.open(&env, "card", 1);
    let wt = Path::new(&a.path);

    // Turn one, snapshotted.
    std::fs::write(wt.join("app.txt"), "good state\n").unwrap();
    std::fs::write(wt.join("keeper.txt"), "worth keeping\n").unwrap();
    let cp = f
        .trees
        .checkpoint(&hr(&env), &a.path, "attempt-1", &a.base_sha, 1)
        .unwrap()
        .unwrap();

    // Turn two ruins it: edits, a deletion, a new file — and one staged
    // entry, standing in for index state that must survive.
    std::fs::write(wt.join("app.txt"), "ruined\n").unwrap();
    std::fs::remove_file(wt.join("keeper.txt")).unwrap();
    std::fs::write(wt.join("stray.txt"), "should vanish\n").unwrap();
    std::fs::write(wt.join("staged.txt"), "staged\n").unwrap();
    git(wt, &["add", "staged.txt"]);

    f.trees.restore_checkpoint(&hr(&env), &a.path, &cp.sha).unwrap();

    assert_eq!(std::fs::read_to_string(wt.join("app.txt")).unwrap(), "good state\n");
    assert_eq!(
        std::fs::read_to_string(wt.join("keeper.txt")).unwrap(),
        "worth keeping\n",
        "a deleted file must come back"
    );
    assert!(!wt.join("stray.txt").exists(), "a post-snapshot file must go");
    // staged.txt was not in the snapshot, so its worktree copy goes — but
    // the index still holds it: restore never reaches into the index.
    assert!(!wt.join("staged.txt").exists());
    let index = git(wt, &["ls-files", "--cached"]);
    assert!(
        index.lines().any(|l| l == "staged.txt"),
        "restore touched the index"
    );

    // Restoring to base is the same act with the free zeroth checkpoint.
    f.trees.restore_checkpoint(&hr(&env), &a.path, &a.base_sha).unwrap();
    assert_eq!(std::fs::read_to_string(wt.join("app.txt")).unwrap(), "one\n");
    assert!(!wt.join("keeper.txt").exists());
}

/* ----------------------------- parked ------------------------------ */

/// The resume half of parking: the worktree grows back onto the existing
/// branch at its exact old path, and the shelf checkpoint brings back the
/// uncommitted work the removal could not keep.
#[test]
fn a_parked_worktree_reattaches_at_its_old_path_and_the_shelf_comes_down() {
    let env = env();
    let f = Fixture::new("parked");
    let a = f.open(&env, "card", 1);
    let wt = Path::new(&a.path);

    // Mid-flight work: an edit and a brand-new file, neither committed.
    std::fs::write(wt.join("app.txt"), "half done\n").unwrap();
    std::fs::write(wt.join("notes.txt"), "todo\n").unwrap();
    let shelf = f
        .trees
        .checkpoint(&hr(&env), &a.path, "attempt-1", &a.base_sha, 1)
        .unwrap()
        .unwrap();

    // Park: the ground goes back, the branch and the refs stay.
    f.trees.remove(&hr(&env), &f.repo_s(), &a.path).unwrap();
    assert!(!wt.exists());
    let refs = git(f.repo.as_path(), &["for-each-ref", "refs/marol/checkpoints"]);
    assert!(refs.contains("attempt-1"), "the shelf must survive the removal");

    // Resume: same path, same branch, then the shelf restores the content.
    f.trees.attach(&hr(&env), &f.repo_s(), &a.path, &a.branch).unwrap();
    assert_eq!(git(wt, &["rev-parse", "--abbrev-ref", "HEAD"]), a.branch);
    // Before the restore the new file is missing — the branch tip never had it.
    assert!(!wt.join("notes.txt").exists());
    f.trees.restore_checkpoint(&hr(&env), &a.path, &shelf.sha).unwrap();
    assert_eq!(std::fs::read_to_string(wt.join("app.txt")).unwrap(), "half done\n");
    assert_eq!(std::fs::read_to_string(wt.join("notes.txt")).unwrap(), "todo\n");
}

/// The path is not negotiable, and neither is honesty about it: a path
/// something else occupies is refused, never adopted.
#[test]
fn attach_refuses_an_occupied_path_and_a_missing_branch() {
    let env = env();
    let f = Fixture::new("attach-refuse");
    let a = f.open(&env, "card", 1);
    f.trees.remove(&hr(&env), &f.repo_s(), &a.path).unwrap();

    std::fs::create_dir_all(&a.path).unwrap();
    let err = f
        .trees
        .attach(&hr(&env), &f.repo_s(), &a.path, &a.branch)
        .expect_err("an occupied path must be refused");
    assert!(err.to_string().contains("already exists"), "unhelpful error: {err}");
    std::fs::remove_dir_all(&a.path).unwrap();

    let err = f
        .trees
        .attach(&hr(&env), &f.repo_s(), &a.path, "marol/never-was")
        .expect_err("a missing branch must be refused");
    assert!(err.to_string().contains("no longer has the branch"), "unhelpful error: {err}");
}

/// A parked attempt's frozen diff: base against the shelf, straight from
/// the object store — tracked and untracked work alike, no worktree needed.
#[test]
fn the_range_diff_reads_a_parked_attempts_work_without_a_worktree() {
    let env = env();
    let f = Fixture::new("range-diff");
    let a = f.open(&env, "card", 1);
    let wt = Path::new(&a.path);
    std::fs::write(wt.join("app.txt"), "changed\n").unwrap();
    std::fs::write(wt.join("fresh.txt"), "new file\n").unwrap();
    let shelf = f
        .trees
        .checkpoint(&hr(&env), &a.path, "attempt-1", &a.base_sha, 1)
        .unwrap()
        .unwrap();
    f.trees.remove(&hr(&env), &f.repo_s(), &a.path).unwrap();

    let diff = f
        .trees
        .diff_range(&hr(&env), &f.repo_s(), &a.base_sha, &shelf.sha, "")
        .unwrap();
    assert!(diff.contains("+changed"));
    assert!(diff.contains("fresh.txt"));
    assert!(diff.contains("+new file"));
}
