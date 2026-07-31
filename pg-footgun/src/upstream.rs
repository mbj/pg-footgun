use std::path::{Path, PathBuf};

use git_proc::Build;
use git_proc::commit_id::CommitId;
use git_proc::commit_ish::CommitIsh;

use crate::Error;

pub const REPO_NAME: &str = "postgres";

pub const UPSTREAM_URL: &str = "https://github.com/postgres/postgres.git";

/// Make `path` absolute (lexically). Paths handed to `git` as arguments while a
/// `-C`/working dir is set must be absolute, else git resolves them against that
/// dir (which silently put worktrees inside the bare clone).
pub fn absolute(path: &Path) -> Result<PathBuf, Error> {
    std::path::absolute(path).map_err(|source| Error::Io {
        action: "resolve",
        path: path.to_owned(),
        source,
    })
}

#[must_use]
pub fn bare_path(target_dir: &Path) -> PathBuf {
    target_dir.join(format!("{REPO_NAME}.git"))
}

/// The `upstream/<name>` subpath shared by the checkout and patch trees, keeping
/// them exact mirrors. `<name>` is the rev without its `upstream/` prefix.
#[must_use]
pub fn tree_subpath(git_ref: &str) -> PathBuf {
    let name = git_ref.strip_prefix("upstream/").unwrap_or(git_ref);
    Path::new("upstream").join(name)
}

#[must_use]
pub fn checkout_path(target_dir: &Path, git_ref: &str) -> PathBuf {
    target_dir.join("checkout").join(tree_subpath(git_ref))
}

/// Idempotent: init the bare clone if missing, (re)configure the `upstream`
/// remote, and fetch.
pub async fn setup_bare(target_dir: &Path) -> Result<PathBuf, Error> {
    let target_dir = absolute(target_dir)?;
    let bare = bare_path(&target_dir);

    if bare.join("HEAD").is_file() {
        log::info!("Bare clone already present at {}", bare.display());
    } else {
        log::info!("Initializing bare clone at {}", bare.display());
        git_proc::init::new()
            .bare()
            .directory(&bare)
            .status()
            .await?;
    }

    configure_remote(&bare, "upstream", UPSTREAM_URL).await?;

    log::info!("Fetching upstream");
    git_proc::fetch::new()
        .repo_path(&bare)
        .all()
        .status()
        .await?;

    log::info!("Bare clone ready at {}", bare.display());
    Ok(bare)
}

async fn configure_remote(bare: &Path, name: &str, url: &str) -> Result<(), Error> {
    log::info!("Configuring remote {name} -> {url}");

    git_proc::config::new(&format!("remote.{name}.url"))
        .repo_path(bare)
        .value(url)
        .status()
        .await?;

    git_proc::config::new(&format!("remote.{name}.fetch"))
        .repo_path(bare)
        .value(&format!("+refs/heads/*:refs/remotes/{name}/*"))
        .status()
        .await?;

    Ok(())
}

/// Set up the bare clone, then check out `commit_ish`. Idempotent: an existing
/// worktree is hard-reset to the resolved commit rather than recreated.
pub async fn checkout(target_dir: &Path, commit_ish: CommitIsh<'_>) -> Result<PathBuf, Error> {
    setup_bare(target_dir).await?;
    checkout_worktree(target_dir, commit_ish).await
}

/// Create-or-reset the worktree (the bare clone must already exist). The rev is
/// resolved to a concrete commit so the checkout is detached, avoiding git's
/// DWIM branch creation.
async fn checkout_worktree(target_dir: &Path, commit_ish: CommitIsh<'_>) -> Result<PathBuf, Error> {
    let target_dir = absolute(target_dir)?;
    let git_ref = crate::target::revspec(commit_ish);

    let bare = bare_path(&target_dir);
    let worktree = checkout_path(&target_dir, git_ref);
    let commit = resolve_commit(&bare, git_ref).await?;

    if worktree.join(".git").exists() {
        log::info!(
            "Resetting {git_ref} worktree to {:.12} at {}",
            commit.as_str(),
            worktree.display()
        );
        reset_worktree(&worktree, commit.as_str()).await?;
    } else {
        log::info!(
            "Checking out {git_ref} ({:.12}) at {}",
            commit.as_str(),
            worktree.display()
        );
        git_proc::worktree::add(&worktree)
            .repo_path(&bare)
            .commit_ish(commit.as_str())
            .status()
            .await?;
    }

    log::info!("Worktree ready at {}", worktree.display());
    Ok(worktree)
}

/// Reset tracked files to `commit`, clearing any half-applied `am`. No
/// `git clean`, so untracked build output survives for an incremental rebuild.
pub async fn reset_worktree(worktree: &Path, commit: &str) -> Result<(), Error> {
    let _ = cmd_proc::Command::new("git")
        .option("-C", worktree)
        .arguments(["am", "--abort"])
        .stderr_capture()
        .stdout_capture()
        .accept_nonzero_exit()
        .run()
        .await?;

    cmd_proc::Command::new("git")
        .option("-C", worktree)
        .arguments(["reset", "--hard", commit])
        .status()
        .await?;

    Ok(())
}

/// Apply the patches, then `./configure && make` (incremental across runs).
pub async fn build(
    target_dir: &Path,
    patches_dir: &Path,
    commit_ish: CommitIsh<'_>,
) -> Result<(), Error> {
    crate::patches::apply(target_dir, patches_dir, commit_ish).await?;
    compile(target_dir, commit_ish, None).await
}

/// Apply the patches, then build and run the regression suite (`make check`).
pub async fn test(
    target_dir: &Path,
    patches_dir: &Path,
    commit_ish: CommitIsh<'_>,
) -> Result<(), Error> {
    crate::patches::apply(target_dir, patches_dir, commit_ish).await?;
    compile(target_dir, commit_ish, Some("check")).await
}

async fn compile(
    target_dir: &Path,
    commit_ish: CommitIsh<'_>,
    make_target: Option<&str>,
) -> Result<(), Error> {
    let target_dir = absolute(target_dir)?;
    let git_ref = crate::target::revspec(commit_ish);

    let worktree = checkout_path(&target_dir, git_ref);

    log::info!("Configuring {}", worktree.display());
    // `--with-libxml`: below 17 the RTE_TABLEFUNC regress case uses XMLTABLE
    // (JSON_TABLE is 17+), which needs libxml at runtime.
    cmd_proc::Command::new(worktree.join("configure"))
        .working_directory(&worktree)
        .argument("--with-libxml")
        .status()
        .await?;

    let jobs = std::thread::available_parallelism().map_or(1, |value| value.get());
    log::info!(
        "Running `make{}` -j{jobs} in {}",
        make_target.map_or(String::new(), |target| format!(" {target}")),
        worktree.display()
    );
    cmd_proc::Command::new("make")
        .working_directory(&worktree)
        .argument("-j")
        .argument(jobs.to_string())
        .optional_argument(crate::target::build_copt(commit_ish).map(|copt| format!("COPT={copt}")))
        .optional_argument(make_target)
        .status()
        .await?;

    log::info!("Done in {}", worktree.display());
    Ok(())
}

async fn resolve_commit(bare: &Path, git_ref: &str) -> Result<CommitId, git_proc::Error> {
    let output = git_proc::rev_parse::new()
        .repo_path(bare)
        .verify()
        .rev(&format!("{git_ref}^{{commit}}"))
        .build()?
        .stdout_capture()
        .string()
        .await?;

    Ok(output
        .trim()
        .parse()
        .expect("rev-parse --verify yields a valid commit id"))
}
