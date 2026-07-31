//! Build targets: a [`Target`] is a [`Platform`] and a [`Version`], named
//! `upstream-18.4` / `debian-18.4`.

use std::fmt;

use git_proc::branch::Branch;
use git_proc::commit_ish::CommitIsh;
use git_proc::tag::Tag;

// `upstream/master` is a `Branch`, not a bare `master`: the latter does not
// resolve in a bare clone, where branches live under `refs/remotes/*`.
static REL_15_10: Tag = Tag::from_static_or_panic("REL_15_10");
static REL_15_12: Tag = Tag::from_static_or_panic("REL_15_12");
static REL_15_17: Tag = Tag::from_static_or_panic("REL_15_17");
static REL_15_18: Tag = Tag::from_static_or_panic("REL_15_18");
static REL_16_14: Tag = Tag::from_static_or_panic("REL_16_14");
static REL_17_10: Tag = Tag::from_static_or_panic("REL_17_10");
static REL_18_3: Tag = Tag::from_static_or_panic("REL_18_3");
static REL_18_4: Tag = Tag::from_static_or_panic("REL_18_4");
static REL_19_BETA2: Tag = Tag::from_static_or_panic("REL_19_BETA2");
static UPSTREAM_MASTER: Branch = Branch::from_static_or_panic("upstream/master");

/// `upstream` builds from a git checkout; `debian` rebuilds the PGDG package.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Upstream,
    Debian,
}

impl Platform {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Upstream => "upstream",
            Platform::Debian => "debian",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A supported PostgreSQL version. `Beta*`/`Master` are upstream-only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Version {
    V15_10,
    V15_12,
    V15_17,
    V15_18,
    V16_14,
    V17_10,
    V18_3,
    V18_4,
    Beta19_2,
    Master,
}

impl Version {
    const ALL: &'static [Version] = &[
        Version::V15_10,
        Version::V15_12,
        Version::V15_17,
        Version::V15_18,
        Version::V16_14,
        Version::V17_10,
        Version::V18_3,
        Version::V18_4,
        Version::Beta19_2,
        Version::Master,
    ];

    /// The release `(major, minor)`, or `None` for `Master`/betas.
    #[must_use]
    pub fn release(self) -> Option<(u8, u8)> {
        Some(match self {
            Version::V15_10 => (15, 10),
            Version::V15_12 => (15, 12),
            Version::V15_17 => (15, 17),
            Version::V15_18 => (15, 18),
            Version::V16_14 => (16, 14),
            Version::V17_10 => (17, 10),
            Version::V18_3 => (18, 3),
            Version::V18_4 => (18, 4),
            Version::Beta19_2 | Version::Master => return None,
        })
    }

    /// `18.4` (release), `19beta2`, `master`.
    #[must_use]
    pub fn name(self) -> String {
        if let Some((major, minor)) = self.release() {
            format!("{major}.{minor}")
        } else {
            match self {
                Version::Beta19_2 => "19beta2",
                Version::Master => "master",
                _ => unreachable!("non-release versions are only Master/beta"),
            }
            .to_owned()
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Version> {
        Version::ALL
            .iter()
            .copied()
            .find(|version| version.name() == name)
    }

    /// The upstream git ref.
    #[must_use]
    pub fn commit_ish(self) -> CommitIsh<'static> {
        match self {
            Version::V15_10 => CommitIsh::Tag(&REL_15_10),
            Version::V15_12 => CommitIsh::Tag(&REL_15_12),
            Version::V15_17 => CommitIsh::Tag(&REL_15_17),
            Version::V15_18 => CommitIsh::Tag(&REL_15_18),
            Version::V16_14 => CommitIsh::Tag(&REL_16_14),
            Version::V17_10 => CommitIsh::Tag(&REL_17_10),
            Version::V18_3 => CommitIsh::Tag(&REL_18_3),
            Version::V18_4 => CommitIsh::Tag(&REL_18_4),
            Version::Beta19_2 => CommitIsh::Tag(&REL_19_BETA2),
            Version::Master => CommitIsh::Branch(&UPSTREAM_MASTER),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// A [`Platform`] and a [`Version`], named `upstream-18.4` / `debian-18.4`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Target {
    platform: Platform,
    version: Version,
}

impl Target {
    #[must_use]
    pub fn new(platform: Platform, version: Version) -> Self {
        Self { platform, version }
    }

    #[must_use]
    pub fn platform(self) -> Platform {
        self.platform
    }

    #[must_use]
    pub fn version(self) -> Version {
        self.version
    }

    #[must_use]
    pub fn commit_ish(self) -> CommitIsh<'static> {
        self.version.commit_ish()
    }

    #[must_use]
    pub fn name(self) -> String {
        format!("{}-{}", self.platform, self.version.name())
    }

    /// Parse `upstream-18.4` / `debian-18.4`; rejects combinations that do not
    /// build (e.g. `debian-master`).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Target> {
        let (platform, rest) = if let Some(rest) = name.strip_prefix("upstream-") {
            (Platform::Upstream, rest)
        } else if let Some(rest) = name.strip_prefix("debian-") {
            (Platform::Debian, rest)
        } else {
            return None;
        };
        let version = Version::from_name(rest)?;
        valid_combo(platform, version).then(|| Target::new(platform, version))
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.platform, self.version)
    }
}

#[must_use]
pub fn revspec(commit_ish: CommitIsh<'_>) -> &str {
    match commit_ish {
        CommitIsh::Branch(branch) => branch.as_str(),
        CommitIsh::Tag(tag) => tag.as_str(),
        CommitIsh::Commit(commit) => commit.as_str(),
    }
}

/// Whether `version` builds on `platform` (Debian only builds releases).
#[must_use]
fn valid_combo(platform: Platform, version: Version) -> bool {
    platform == Platform::Upstream || version.release().is_some()
}

/// Every valid target, upstream first then Debian.
pub fn all_targets() -> impl Iterator<Item = Target> {
    [Platform::Upstream, Platform::Debian]
        .into_iter()
        .flat_map(|platform| {
            Version::ALL
                .iter()
                .copied()
                .filter(move |&version| valid_combo(platform, version))
                .map(move |version| Target::new(platform, version))
        })
}

/// One [`Target`] or `all`. A command narrows it via [`Selection::resolve`].
#[derive(Clone, Copy, Debug)]
pub enum Selection {
    All,
    One(Target),
}

impl Selection {
    /// The targets to process, restricted to `accepted` platforms. `all` expands
    /// to every valid target on them; a single target must be on one of them.
    pub fn resolve(self, accepted: &[Platform]) -> Result<Vec<Target>, PlatformNotAccepted> {
        match self {
            Selection::All => Ok(all_targets()
                .filter(|target| accepted.contains(&target.platform()))
                .collect()),
            Selection::One(target) if accepted.contains(&target.platform()) => Ok(vec![target]),
            Selection::One(target) => Err(PlatformNotAccepted {
                target: target.name(),
                accepted: accepted
                    .iter()
                    .map(|platform| platform.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("target {target} is not valid here; this command accepts: {accepted}")]
pub struct PlatformNotAccepted {
    target: String,
    accepted: String,
}

/// clap `value_parser` for a target name or `all`.
pub fn parse_selection(name: &str) -> Result<Selection, String> {
    if name == "all" {
        Ok(Selection::All)
    } else {
        Target::from_name(name)
            .map(Selection::One)
            .ok_or_else(|| format!("unsupported target {name:?}; supported: {}", supported()))
    }
}

#[must_use]
pub fn supported() -> String {
    all_targets()
        .map(|target| target.name())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Extra `make COPT` flags. `REL_15_10` predates upstream's C23 `<stdbool.h>`
/// fix, so `-std=gnu17` lets it build with a C23-default host compiler.
#[must_use]
pub fn build_copt(commit_ish: CommitIsh<'_>) -> Option<&'static str> {
    match revspec(commit_ish) {
        "REL_15_10" => Some("-std=gnu17"),
        _ => None,
    }
}
