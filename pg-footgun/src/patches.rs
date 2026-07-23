use std::path::{Path, PathBuf};

use cmd_proc::Command;
use git_proc::commit_ish::CommitIsh;

use crate::{Error, target, upstream};

/// Check out `rev` (which resets its worktree to pristine), then `git am --3way`
/// the committed `patches/upstream/<name>/*.patch` onto it. On a release checkout
/// `--3way` leaves any conflict for manual resolution (`git am --continue`).
/// Idempotent (re-running picks up edited patches); no branch is touched.
pub async fn apply(
    target_dir: &Path,
    patches_dir: &Path,
    commit_ish: CommitIsh<'_>,
) -> Result<(), Error> {
    let worktree = upstream::checkout(target_dir, commit_ish).await?;

    let patches_dir = upstream::absolute(patches_dir)?;
    let git_ref = target::revspec(commit_ish);
    let dir = patches_dir.join(upstream::tree_subpath(git_ref));
    let files = find_patches(&dir)?;
    if files.is_empty() {
        return Err(Error::NoPatches(dir));
    }

    log::info!(
        "Applying {} patch(es) from {} onto {}",
        files.len(),
        dir.display(),
        worktree.display()
    );
    // `git am` records a committer; pin a fixed identity via `-c` so it works
    // without a configured git user (e.g. in CI). The author comes from the
    // patch's `From:`; this commit is a transient build artifact, never pushed.
    Command::new("git")
        .option("-C", &worktree)
        .option("-c", "user.name=pg-footgun")
        .option("-c", "user.email=pg-footgun@localhost")
        .argument("am")
        .argument("--3way")
        .arguments(&files)
        .status()
        .await?;

    log::info!("Patches applied at {}", worktree.display());
    Ok(())
}

pub struct Entry {
    /// `<context>/<name>` subpath, mirroring `target/checkout/<target>`.
    pub target: PathBuf,
    pub footgun: String,
    pub path: PathBuf,
}

pub fn list(patches_dir: &Path) -> Result<Vec<Entry>, Error> {
    Ok(find_patches(patches_dir)?
        .into_iter()
        .map(|path| {
            let target = path
                .strip_prefix(patches_dir)
                .ok()
                .and_then(|relative| relative.parent())
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let footgun = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            Entry {
                target,
                footgun,
                path,
            }
        })
        .collect())
}

/// Recursively collect `*.patch` files under `root`, sorted (a missing `root`
/// yields an empty list). Shared by `apply` (one target dir), `list`, and
/// `debian::apply` (register the same footgun patches into the quilt series).
pub(crate) fn find_patches(root: &Path) -> Result<Vec<PathBuf>, Error> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), Error> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(Error::Io {
                    action: "read",
                    path: dir.to_owned(),
                    source,
                });
            }
        };

        for entry in entries {
            let path = entry
                .map_err(|source| Error::Io {
                    action: "read",
                    path: dir.to_owned(),
                    source,
                })?
                .path();

            if path.is_dir() {
                walk(&path, files)?;
            } else if path
                .extension()
                .is_some_and(|extension| extension == "patch")
            {
                files.push(path);
            }
        }

        Ok(())
    }

    let mut files = Vec::new();
    walk(root, &mut files)?;
    files.sort();
    Ok(files)
}
