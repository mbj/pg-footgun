use std::path::{Path, PathBuf};

use ociman::{Definition, Mount, Reference};
use rustix::process::{Gid, Uid};

use crate::Error;
use crate::target::Version;

const WORK: &str = "/work";
const PGDG_SRC_DEST: &str = "/etc/apt/sources.list.d/pgdg-src.list";

// Fixed non-root in-container build identity (the uid namespace makes the outer
// uid irrelevant). See mrs `pg-ephemeral`'s `transparent_user`.
const BUILD_USER: &str = "pgbuild";
const BUILD_HOME: &str = "/home/pgbuild";
const BUILD_UID: u32 = 2000;

/// Footgun-image release counter, the `N` in the `-pg-footgun-N` image tag. Bump
/// when the footgun patch set changes (rebuilds/republishes every minor); a new
/// PG minor reuses the current `N`.
const FOOTGUN_RELEASE: u8 = 0;

/// Registry repository the patched images are published to. The base still comes
/// from the official `docker.io/library/postgres` (see [`Package::image`]).
const IMAGE_REPO: &str = "ghcr.io/mbj/postgres";

/// A Debian source package pinned to a precise minor (e.g. `18.4`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Package {
    major: u8,
    minor: u8,
}

impl Package {
    /// The source package name (`postgresql-18`) — major-only; `apt-get source`
    /// takes this and pins the minor separately (see `version`).
    #[must_use]
    pub fn name(self) -> String {
        format!("postgresql-{}", self.major)
    }

    #[must_use]
    pub fn major(self) -> u8 {
        self.major
    }

    /// The precise version, e.g. `18.4`.
    #[must_use]
    pub fn version(self) -> String {
        format!("{}.{}", self.major, self.minor)
    }

    #[must_use]
    pub fn image(self) -> String {
        format!("docker.io/library/postgres:{}.{}", self.major, self.minor)
    }

    /// The immutable patched image reference, e.g.
    /// `ghcr.io/mbj/postgres:18.4-pg-footgun-0`.
    #[must_use]
    pub fn footgun_image(self) -> String {
        format!(
            "{IMAGE_REPO}:{}-pg-footgun-{FOOTGUN_RELEASE}",
            self.version()
        )
    }

    /// The moving tag tracking the newest patch set for this minor, e.g.
    /// `ghcr.io/mbj/postgres:18.4-pg-footgun`.
    #[must_use]
    pub fn footgun_image_moving(self) -> String {
        format!("{IMAGE_REPO}:{}-pg-footgun", self.version())
    }

    /// The upstream patch tag (`REL_18_4`), matching `patches/upstream/`.
    #[must_use]
    pub fn tag(self) -> String {
        format!("REL_{}_{}", self.major, self.minor)
    }
}

impl TryFrom<Version> for Package {
    type Error = NotADebianTarget;

    fn try_from(version: Version) -> Result<Self, Self::Error> {
        let (major, minor) = version.release().ok_or(NotADebianTarget(version))?;
        Ok(Self { major, minor })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("no Debian target debian-{}; master and betas are upstream-only", .0.name())]
pub struct NotADebianTarget(Version);

/// Unpack the PGDG source `package` into `<target_dir>/debian/<version>` via
/// `apt-get source` in the pinned `postgres:<version>` container. Returns the
/// host directory holding the tree.
pub async fn unpack(
    target_dir: &Path,
    pgdg_src_list: &Path,
    package: Package,
) -> Result<PathBuf, Error> {
    let name = package.name();
    // PGDG's index keeps several minors and `apt-get source` takes the minor as
    // the version, so pin the exact requested one.
    let source_spec = format!("{name}={}", package.version());

    // Fully qualified so podman (no short-name resolution by default) accepts it.
    let base = package.image();
    let reference: Reference = base
        .parse()
        .expect("`docker.io/library/postgres:<version>` is a valid image reference");

    let dest = target_dir.join("debian").join(package.version());
    std::fs::create_dir_all(&dest).map_err(|source| Error::Io {
        action: "create",
        path: dest.clone(),
        source,
    })?;

    // Idempotent: reuse the already-unpacked per-version tree. Remove
    // `target/debian/<version>` to force a fresh fetch.
    if let Ok(found) = unpacked_version(&dest, &name) {
        log::info!("{name} {found} already unpacked at {}", dest.display());
        return Ok(dest);
    }

    let dest_abs = canonicalize(&dest)?;
    let src_list_abs = canonicalize(pgdg_src_list)?;

    let backend = ociman::backend::Selection::Auto.resolve().await?;
    // Extract as the "host user" so the tree is host-owned and later host-side
    // steps can edit it (mrs `transparent_user`, not a post-hoc chown).
    let (host_uid, host_gid) = host_user(&backend);

    log::info!(
        "Unpacking Debian source {name} in {base} -> {}",
        dest.display()
    );

    let result: Result<(), cmd_proc::CommandError> = Definition::new(backend, reference)
        .entrypoint("sleep")
        .argument("infinity")
        .mount(Mount::from(format!(
            "type=bind,src={},dst={WORK}",
            dest_abs.display()
        )))
        .mount(Mount::from(format!(
            "type=bind,src={},dst={PGDG_SRC_DEST}",
            src_list_abs.display()
        )))
        .workdir(WORK)
        .remove()
        .with_container(async |container| {
            container_exec(container, "apt-get", ["update"]).await?;
            // `dpkg-dev` provides the `dpkg-source` the base image lacks.
            container_exec(
                container,
                "apt-get",
                ["install", "--yes", "--no-install-recommends", "dpkg-dev"],
            )
            .await?;
            container_exec_as(
                container,
                host_uid,
                host_gid,
                "apt-get",
                ["source", source_spec.as_str()],
            )
            .await?;
            Ok(())
        })
        .await?;
    result?;

    let found = unpacked_version(&dest, &name)?;
    log::info!("Unpacked {name} {found} at {}", dest.display());

    Ok(dest)
}

/// Unpack, then register the footgun patch(es) into the Debian quilt series and
/// `quilt push -a` to confirm they apply atop Debian's own patches. The patch
/// comes from `patches/upstream/REL_<version>`, shared with the upstream build.
/// Idempotent. Returns the unpacked source directory.
pub async fn apply(
    target_dir: &Path,
    patches_dir: &Path,
    pgdg_src_list: &Path,
    package: Package,
) -> Result<PathBuf, Error> {
    let dest = unpack(target_dir, pgdg_src_list, package).await?;
    let name = package.name();
    let version = package.version();
    let source_dir = dest.join(format!("{name}-{version}"));

    // The requested version pins the patch directly, mirroring the upstream side.
    let tag = package.tag();
    let footgun_dir =
        crate::upstream::absolute(patches_dir)?.join(crate::upstream::tree_subpath(&tag));
    let footguns = crate::patches::find_patches(&footgun_dir)?;
    if footguns.is_empty() {
        return Err(Error::NoPatches(footgun_dir));
    }

    register_quilt_patches(&source_dir, &footguns)?;

    let base = package.image();
    let reference: Reference = base
        .parse()
        .expect("`docker.io/library/postgres:<version>` is a valid image reference");
    let source_abs = canonicalize(&source_dir)?;
    let backend = ociman::backend::Selection::Auto.resolve().await?;

    log::info!(
        "Applying {} footgun patch(es) for {name} {version} in {base}",
        footguns.len()
    );

    let result: Result<(), cmd_proc::CommandError> = Definition::new(backend, reference)
        .entrypoint("sleep")
        .argument("infinity")
        .mount(Mount::from(format!(
            "type=bind,src={},dst={WORK}",
            source_abs.display()
        )))
        .workdir(WORK)
        .remove()
        .with_container(async |container| {
            container_exec(container, "apt-get", ["update"]).await?;
            container_exec(
                container,
                "apt-get",
                ["install", "--yes", "--no-install-recommends", "quilt"],
            )
            .await?;
            // `.pc/.quilt_patches` points quilt at debian/patches. Reset to
            // pristine first (tolerating "no patches applied"), then push the
            // whole series fresh so `push` always does real work: a nonzero exit
            // then means a real conflict, not the "series fully applied" status
            // quilt returns (and which would otherwise break idempotency).
            // `-a` (apply/remove all) has no long form in quilt.
            container_try_exec(container, "quilt", ["pop", "-a"]).await?;
            container_exec(container, "quilt", ["push", "-a"]).await?;
            Ok(())
        })
        .await?;
    result?;

    log::info!("Footgun patches applied at {}", source_dir.display());

    Ok(source_dir)
}

/// Register + apply the footgun patch(es), then `dpkg-buildpackage` the package
/// in a `postgres:<major>` container, producing the patched `.deb`s next to the
/// source. Debian's `postgresql.mk` runs the upstream regression suite during
/// the build, so this also exercises the footgun on the Debian-patched tree.
pub async fn build(
    target_dir: &Path,
    patches_dir: &Path,
    pgdg_src_list: &Path,
    package: Package,
) -> Result<(), Error> {
    apply(target_dir, patches_dir, pgdg_src_list, package).await?;

    let name = package.name();
    let version = package.version();
    let dest = target_dir.join("debian").join(&version);

    let subdir = format!("{name}-{version}");
    let src_in_work = format!("{WORK}/{subdir}");
    let src_in_home = format!("{BUILD_HOME}/{subdir}");
    let build_uid_arg = BUILD_UID.to_string();
    let build_owner = format!("{BUILD_USER}:{BUILD_USER}");
    let build_uid = Uid::from_raw(BUILD_UID);
    let build_gid = Gid::from_raw(BUILD_UID);
    let jobs = std::thread::available_parallelism().map_or(1, |value| value.get());
    // `--jobs-force` is the only dpkg-buildpackage option that puts `-jN` + a
    // jobserver into MAKEFLAGS, which is the only thing postgresql.mk's hand-rolled
    // `$(MAKE) -C build/src all` (bypassing dh_auto_build) picks up.
    let jobs_force = format!("--jobs-force={jobs}");

    let base = package.image();
    let reference: Reference = base
        .parse()
        .expect("`docker.io/library/postgres:<version>` is a valid image reference");
    let dest_abs = canonicalize(&dest)?;
    let backend = ociman::backend::Selection::Auto.resolve().await?;

    log::info!(
        "Building {name} {version} .debs in {base} -> {}",
        dest.display()
    );

    let result: Result<(), cmd_proc::CommandError> = Definition::new(backend, reference)
        .entrypoint("sleep")
        .argument("infinity")
        .mount(Mount::from(format!(
            "type=bind,src={},dst={WORK}",
            dest_abs.display()
        )))
        // The default seccomp profile ERRNO-blocks `move_pages`, which PG 18+'s
        // `numa` regression test hits (its `pg_numa_available()` probe only
        // guards `get_mempolicy`); unconfined lets the suite pass, matching how
        // Debian's own sbuild chroots build without a seccomp filter.
        .security_option("seccomp=unconfined")
        .workdir(WORK)
        .remove()
        .with_container(async |container| {
            container_exec(container, "apt-get", ["update"]).await?;
            // Unprivileged build user (see BUILD_UID).
            container_exec(
                container,
                "useradd",
                [
                    "--uid",
                    build_uid_arg.as_str(),
                    "--user-group",
                    "--create-home",
                    BUILD_USER,
                ],
            )
            .await?;
            // Copy the patched source off the bind mount: the build runs
            // unprivileged (its regression suite's initdb refuses root) and so
            // can't own the mount. dpkg-buildpackage writes the .debs beside it.
            container_exec(
                container,
                "cp",
                ["--archive", src_in_work.as_str(), src_in_home.as_str()],
            )
            .await?;
            // Build-Depends come from the copied source's debian/control.
            container_exec(
                container,
                "apt-get",
                ["build-dep", "--yes", src_in_home.as_str()],
            )
            .await?;
            // fakeroot: dpkg-buildpackage's packaging steps need it when non-root.
            // catatonit: a reaping init used as the build's exec-root (below).
            container_exec(
                container,
                "apt-get",
                [
                    "install",
                    "--yes",
                    "--no-install-recommends",
                    "fakeroot",
                    "catatonit",
                ],
            )
            .await?;
            container_exec(
                container,
                "chown",
                ["--recursive", build_owner.as_str(), BUILD_HOME],
            )
            .await?;
            // Build unprivileged; `--jobs-force` parallelises the compile (see
            // jobs_force); postgresql.mk runs the regression suite.
            //
            // Run it under `catatonit` as the exec-root. `podman exec` makes the
            // exec'd process a subreaper, so daemonized grandchildren (the TAP
            // suite's postmasters, orphaned once `pg_ctl` exits) reparent to it
            // rather than to PID 1. dpkg-buildpackage doesn't reap them, so a
            // `kill -9`'d postmaster (e.g. in 017_shm) would linger as a zombie
            // whose pid still answers `kill(pid,0)`, tripping the postmaster.pid
            // interlock. catatonit reaps them, as a host's init would.
            container
                .exec("catatonit")
                .arguments([
                    "--",
                    "dpkg-buildpackage",
                    "--build=binary",
                    "--no-sign",
                    jobs_force.as_str(),
                ])
                .user(build_uid, build_gid)
                .workdir(src_in_home.as_str())
                .environment_variable(
                    cmd_proc::EnvVariableName::from_static_or_panic("HOME"),
                    BUILD_HOME,
                )
                .status()
                .await?;
            // Materialise the .debs onto the bind mount as root: in rootless
            // docker/podman container root maps to the host user, so they come
            // back host-owned (external is never internal, so root is correct
            // here, not a mirrored host uid).
            container_exec(
                container,
                "find",
                [
                    BUILD_HOME,
                    "-maxdepth",
                    "1",
                    "-type",
                    "f",
                    "-name",
                    "*.deb",
                    "-exec",
                    "cp",
                    "--target-directory",
                    WORK,
                    "{}",
                    "+",
                ],
            )
            .await?;
            Ok(())
        })
        .await?;
    result?;

    let debs = built_debs(&dest)?;
    for deb in &debs {
        log::info!("Built {}", deb.display());
    }
    log::info!("Built {} .deb(s) in {}", debs.len(), dest.display());

    Ok(())
}

/// Build the patched OCI image [`Package::footgun_image`] by overlaying the
/// patched server `.deb` onto the official `postgres:<version>` base — the same
/// base, so the ABI matches exactly. Only `postgresql-<major>` carries the
/// footgun (the planner/GUC change); its unpatched siblings already satisfy its
/// dependencies at the identical version. Runs [`build`] first if the `.deb` is
/// missing, otherwise reuses it. Returns the built image reference.
pub async fn image(
    target_dir: &Path,
    patches_dir: &Path,
    pgdg_src_list: &Path,
    package: Package,
) -> Result<String, Error> {
    let dest = target_dir.join("debian").join(package.version());

    // Reuse an existing build (a full build is ~minutes); build if absent.
    let debs = match built_debs(&dest) {
        Ok(debs) if !debs.is_empty() => {
            log::info!("Using existing .debs in {}", dest.display());
            debs
        }
        _ => {
            build(target_dir, patches_dir, pgdg_src_list, package).await?;
            built_debs(&dest)?
        }
    };

    // The patched server package (exact-name match on the `.deb` filename, so
    // `-dev`/`-dbgsym`/`-jit` variants are excluded).
    let prefix = format!("postgresql-{}_", package.major());
    let server = debs
        .iter()
        .find(|deb| {
            deb.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with(&prefix)
        })
        .ok_or_else(|| Error::NoServerDeb {
            package: package.name(),
            path: dest.clone(),
        })?;

    // Stage a clean context holding just the server `.deb` + Dockerfile, so the
    // context upload does not carry the whole unpacked source tree.
    let context = dest.join("image");
    if context.exists() {
        std::fs::remove_dir_all(&context).map_err(|source| Error::Io {
            action: "remove",
            path: context.clone(),
            source,
        })?;
    }
    std::fs::create_dir_all(&context).map_err(|source| Error::Io {
        action: "create",
        path: context.clone(),
        source,
    })?;
    let deb_name = server.file_name().expect("a .deb path has a file name");
    std::fs::copy(server, context.join(deb_name)).map_err(|source| Error::Io {
        action: "copy",
        path: context.join(deb_name),
        source,
    })?;

    let dockerfile = context.join("Dockerfile");
    let contents = format!(
        "FROM {base}\n\
         COPY *.deb /tmp/footgun.deb\n\
         RUN set -eux; \\\n    \
         dpkg --install /tmp/footgun.deb; \\\n    \
         rm --force /tmp/footgun.deb; \\\n    \
         postgres --version\n",
        base = package.image(),
    );
    std::fs::write(&dockerfile, contents).map_err(|source| Error::Io {
        action: "write",
        path: dockerfile.clone(),
        source,
    })?;

    let context_abs = canonicalize(&context)?;
    let dockerfile_abs = canonicalize(&dockerfile)?;
    let tag = package.footgun_image();
    let moving = package.footgun_image_moving();

    let backend = ociman::backend::Selection::Auto.resolve().await?;
    log::info!("Building image {tag} from {}", package.image());
    backend
        .command()
        .argument("build")
        .argument("--tag")
        .argument(&tag)
        .argument("--tag")
        .argument(&moving)
        .argument("--file")
        .argument(&dockerfile_abs)
        .argument(&context_abs)
        .status()
        .await?;

    log::info!("Built image {tag}");
    Ok(tag)
}

/// Build the patched image (reusing existing `.debs`), then push both its
/// immutable `-pg-footgun-N` and moving `-pg-footgun` tags. The registry must
/// already be authenticated (e.g. `docker login ghcr.io`).
pub async fn push(
    target_dir: &Path,
    patches_dir: &Path,
    pgdg_src_list: &Path,
    package: Package,
) -> Result<(), Error> {
    image(target_dir, patches_dir, pgdg_src_list, package).await?;

    let backend = ociman::backend::Selection::Auto.resolve().await?;
    for tag in [package.footgun_image(), package.footgun_image_moving()] {
        let reference: Reference = tag.parse().expect("footgun image reference is valid");
        log::info!("Pushing {tag}");
        backend.push_image(&reference).await?;
    }

    Ok(())
}

/// Collect the `.deb` files produced next to the source tree.
fn built_debs(dest: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut debs = Vec::new();
    let entries = std::fs::read_dir(dest).map_err(|source| Error::Io {
        action: "read",
        path: dest.to_owned(),
        source,
    })?;
    for entry in entries {
        let path = entry
            .map_err(|source| Error::Io {
                action: "read",
                path: dest.to_owned(),
                source,
            })?
            .path();
        if path.extension().is_some_and(|extension| extension == "deb") {
            debs.push(path);
        }
    }
    debs.sort();
    Ok(debs)
}

/// Copy each footgun patch into `debian/patches` (prefixed `pg-footgun-`) and
/// append it to the `series` if not already listed. Host-side file work, so no
/// shell redirection into the series is needed.
fn register_quilt_patches(source_dir: &Path, footguns: &[PathBuf]) -> Result<(), Error> {
    let patches = source_dir.join("debian").join("patches");
    let series_path = patches.join("series");

    let mut series = std::fs::read_to_string(&series_path).map_err(|source| Error::Io {
        action: "read",
        path: series_path.clone(),
        source,
    })?;
    if !series.ends_with('\n') {
        series.push('\n');
    }

    let mut appended = String::new();
    for footgun in footguns {
        let stem = footgun
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        let entry = format!("pg-footgun-{stem}");

        let copied = patches.join(&entry);
        std::fs::copy(footgun, &copied).map_err(|source| Error::Io {
            action: "copy",
            path: copied,
            source,
        })?;

        if !series.lines().any(|line| line.trim() == entry) {
            appended.push_str(&entry);
            appended.push('\n');
        }
    }

    if !appended.is_empty() {
        series.push_str(&appended);
        std::fs::write(&series_path, series).map_err(|source| Error::Io {
            action: "write",
            path: series_path,
            source,
        })?;
    }

    Ok(())
}

/// The "host user" uid/gid to run bind-mount writes as, so they come back owned
/// by the invoking user: root under rootless (container 0 maps to the host
/// user), the host uid under rootful (1:1 mapping). Mirrors mrs `pg-ephemeral`'s
/// `transparent_user`.
fn host_user(backend: &ociman::backend::Backend) -> (Uid, Gid) {
    if backend.is_rootless() {
        (Uid::ROOT, Gid::ROOT)
    } else {
        (rustix::process::getuid(), rustix::process::getgid())
    }
}

/// Run `executable` with `arguments` inside the build container as root.
async fn container_exec<'a>(
    container: &ociman::Container,
    executable: &str,
    arguments: impl IntoIterator<Item = &'a str>,
) -> Result<(), cmd_proc::CommandError> {
    container_exec_as(container, Uid::ROOT, Gid::ROOT, executable, arguments).await
}

/// Like [`container_exec`], but as an explicit `uid:gid` (used with
/// [`host_user`] for bind-mount writes that must land host-owned).
async fn container_exec_as<'a>(
    container: &ociman::Container,
    uid: Uid,
    gid: Gid,
    executable: &str,
    arguments: impl IntoIterator<Item = &'a str>,
) -> Result<(), cmd_proc::CommandError> {
    container
        .exec(executable)
        .arguments(arguments)
        .user(uid, gid)
        .status()
        .await
}

/// Like [`container_exec`], but tolerates a nonzero exit — for commands whose
/// "nothing to do" status is not a failure (e.g. `quilt pop -a` on a tree with
/// no patches applied).
async fn container_try_exec<'a>(
    container: &ociman::Container,
    executable: &str,
    arguments: impl IntoIterator<Item = &'a str>,
) -> Result<(), cmd_proc::CommandError> {
    container
        .exec(executable)
        .arguments(arguments)
        .user(Uid::ROOT, Gid::ROOT)
        .to_cmd_proc_command()
        .stdout_capture()
        .stderr_capture()
        .accept_nonzero_exit()
        .run()
        .await
        .map(|_| ())
}

/// Read the exact version from the unpacked directory name
/// (`postgresql-18-18.4` -> `18.4`) — a post-unpack filesystem signal.
fn unpacked_version(dest: &Path, name: &str) -> Result<String, Error> {
    let prefix = format!("{name}-");

    let entries = std::fs::read_dir(dest).map_err(|source| Error::Io {
        action: "read",
        path: dest.to_owned(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            action: "read",
            path: dest.to_owned(),
            source,
        })?;

        let is_dir = entry
            .file_type()
            .map_err(|source| Error::Io {
                action: "read",
                path: entry.path(),
                source,
            })?
            .is_dir();

        if is_dir
            && let Some(version) = entry
                .file_name()
                .to_str()
                .and_then(|dir_name| dir_name.strip_prefix(&prefix))
        {
            return Ok(version.to_owned());
        }
    }

    Err(Error::SourceNotUnpacked {
        package: name.to_owned(),
        path: dest.to_owned(),
    })
}

fn canonicalize(path: &Path) -> Result<PathBuf, Error> {
    std::fs::canonicalize(path).map_err(|source| Error::Io {
        action: "resolve",
        path: path.to_owned(),
        source,
    })
}
