use std::path::PathBuf;

use crate::Error;
use crate::debian::Package;
use crate::target::{Platform, Selection, Target};

const BOTH: &[Platform] = &[Platform::Upstream, Platform::Debian];
const UPSTREAM_ONLY: &[Platform] = &[Platform::Upstream];
const DEBIAN_ONLY: &[Platform] = &[Platform::Debian];

/// pg-footgun — build and maintain patched PostgreSQL OCI images.
///
/// Commands take a platform-qualified target: `upstream-18.4` / `debian-18.4`
/// (or `all`).
#[derive(Debug, clap::Parser)]
#[command(name = "pg-footgun", version, about, long_about = None)]
pub struct App {
    #[command(subcommand)]
    command: Command,
}

impl App {
    pub async fn run(self) -> Result<(), Error> {
        self.command.run().await
    }
}

#[derive(Debug, clap::Parser)]
enum Command {
    /// Create or update the shared PostgreSQL upstream bare clone.
    SetupBare(SetupBare),
    /// Check out pristine upstream source (`upstream-*` targets).
    Checkout(Checkout),
    /// Download and unpack a PGDG source package (`debian-*` targets).
    Unpack(Unpack),
    /// Apply the committed footgun patch(es) for a target.
    Apply(Apply),
    /// Apply patches and build (Debian also runs the regression suite).
    Build(Build),
    /// Build and run the regression suite.
    Test(Test),
    /// Build the patched OCI image from the Debian `.deb`s (`debian-*` targets).
    Image(Image),
    /// Build and push the patched image to the registry (`debian-*` targets).
    Push(Push),
    /// List the committed footgun patches and the target each applies to.
    ListPatches(ListPatches),
}

impl Command {
    async fn run(self) -> Result<(), Error> {
        match self {
            Self::SetupBare(command) => command.run().await,
            Self::Checkout(command) => command.run().await,
            Self::Unpack(command) => command.run().await,
            Self::Apply(command) => command.run().await,
            Self::Build(command) => command.run().await,
            Self::Test(command) => command.run().await,
            Self::Image(command) => command.run().await,
            Self::Push(command) => command.run().await,
            Self::ListPatches(command) => command.run().await,
        }
    }
}

fn debian_package(target: Target) -> Package {
    Package::try_from(target.version())
        .expect("resolve() yields only release versions for Debian targets")
}

#[derive(Debug, clap::Parser)]
pub struct SetupBare {
    #[arg(long, default_value = "target")]
    target_dir: PathBuf,
}

impl SetupBare {
    pub async fn run(self) -> Result<(), Error> {
        crate::upstream::setup_bare(&self.target_dir).await?;
        Ok(())
    }
}

#[derive(Debug, clap::Parser)]
pub struct Checkout {
    /// Upstream target to check out (e.g. `upstream-18.4`), or `all`.
    #[arg(value_parser = crate::target::parse_selection)]
    selection: Selection,

    #[arg(long, default_value = "target")]
    target_dir: PathBuf,
}

impl Checkout {
    pub async fn run(self) -> Result<(), Error> {
        for target in self.selection.resolve(UPSTREAM_ONLY)? {
            log::info!("Checking out {target}");
            crate::upstream::checkout(&self.target_dir, target.commit_ish()).await?;
        }
        Ok(())
    }
}

#[derive(Debug, clap::Parser)]
pub struct Unpack {
    /// Debian target to unpack (e.g. `debian-18.4`), or `all`.
    #[arg(value_parser = crate::target::parse_selection)]
    selection: Selection,

    #[arg(long, default_value = "target")]
    target_dir: PathBuf,

    /// PGDG `deb-src` sources file mounted into the build container.
    #[arg(long, default_value = "debian/pgdg-src.list")]
    pgdg_src_list: PathBuf,
}

impl Unpack {
    pub async fn run(self) -> Result<(), Error> {
        for target in self.selection.resolve(DEBIAN_ONLY)? {
            log::info!("Unpacking {target}");
            crate::debian::unpack(
                &self.target_dir,
                &self.pgdg_src_list,
                debian_package(target),
            )
            .await?;
        }
        Ok(())
    }
}

#[derive(Debug, clap::Parser)]
pub struct Apply {
    /// Platform-qualified target (e.g. `upstream-18.4`, `debian-18.4`), or `all`.
    #[arg(value_parser = crate::target::parse_selection)]
    selection: Selection,

    #[arg(long, default_value = "target")]
    target_dir: PathBuf,

    #[arg(long, default_value = "patches")]
    patches_dir: PathBuf,

    /// PGDG `deb-src` sources file mounted into the build container.
    #[arg(long, default_value = "debian/pgdg-src.list")]
    pgdg_src_list: PathBuf,
}

impl Apply {
    pub async fn run(self) -> Result<(), Error> {
        for target in self.selection.resolve(BOTH)? {
            log::info!("Applying footgun to {target}");
            match target.platform() {
                Platform::Upstream => {
                    crate::patches::apply(&self.target_dir, &self.patches_dir, target.commit_ish())
                        .await?;
                }
                Platform::Debian => {
                    crate::debian::apply(
                        &self.target_dir,
                        &self.patches_dir,
                        &self.pgdg_src_list,
                        debian_package(target),
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, clap::Parser)]
pub struct Build {
    /// Platform-qualified target (e.g. `upstream-18.4`, `debian-18.4`), or `all`.
    #[arg(value_parser = crate::target::parse_selection)]
    selection: Selection,

    #[arg(long, default_value = "target")]
    target_dir: PathBuf,

    #[arg(long, default_value = "patches")]
    patches_dir: PathBuf,

    /// PGDG `deb-src` sources file mounted into the build container.
    #[arg(long, default_value = "debian/pgdg-src.list")]
    pgdg_src_list: PathBuf,
}

impl Build {
    pub async fn run(self) -> Result<(), Error> {
        for target in self.selection.resolve(BOTH)? {
            log::info!("Building {target}");
            match target.platform() {
                Platform::Upstream => {
                    crate::upstream::build(
                        &self.target_dir,
                        &self.patches_dir,
                        target.commit_ish(),
                    )
                    .await?;
                }
                Platform::Debian => {
                    crate::debian::build(
                        &self.target_dir,
                        &self.patches_dir,
                        &self.pgdg_src_list,
                        debian_package(target),
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, clap::Parser)]
pub struct Test {
    /// Platform-qualified target (e.g. `upstream-18.4`, `debian-18.4`), or `all`.
    #[arg(value_parser = crate::target::parse_selection)]
    selection: Selection,

    #[arg(long, default_value = "target")]
    target_dir: PathBuf,

    #[arg(long, default_value = "patches")]
    patches_dir: PathBuf,

    /// PGDG `deb-src` sources file mounted into the build container.
    #[arg(long, default_value = "debian/pgdg-src.list")]
    pgdg_src_list: PathBuf,
}

impl Test {
    pub async fn run(self) -> Result<(), Error> {
        for target in self.selection.resolve(BOTH)? {
            log::info!("Testing {target}");
            match target.platform() {
                Platform::Upstream => {
                    crate::upstream::test(&self.target_dir, &self.patches_dir, target.commit_ish())
                        .await?;
                }
                // For Debian, the build already runs the full suite.
                Platform::Debian => {
                    crate::debian::build(
                        &self.target_dir,
                        &self.patches_dir,
                        &self.pgdg_src_list,
                        debian_package(target),
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, clap::Parser)]
pub struct Image {
    /// Debian target to build an image for (e.g. `debian-18.4`), or `all`.
    #[arg(value_parser = crate::target::parse_selection)]
    selection: Selection,

    #[arg(long, default_value = "target")]
    target_dir: PathBuf,

    #[arg(long, default_value = "patches")]
    patches_dir: PathBuf,

    /// PGDG `deb-src` sources file mounted into the build container.
    #[arg(long, default_value = "debian/pgdg-src.list")]
    pgdg_src_list: PathBuf,
}

impl Image {
    pub async fn run(self) -> Result<(), Error> {
        for target in self.selection.resolve(DEBIAN_ONLY)? {
            crate::debian::image(
                &self.target_dir,
                &self.patches_dir,
                &self.pgdg_src_list,
                debian_package(target),
            )
            .await?;
        }
        Ok(())
    }
}

#[derive(Debug, clap::Parser)]
pub struct Push {
    /// Debian target to build and push (e.g. `debian-18.4`), or `all`.
    #[arg(value_parser = crate::target::parse_selection)]
    selection: Selection,

    #[arg(long, default_value = "target")]
    target_dir: PathBuf,

    #[arg(long, default_value = "patches")]
    patches_dir: PathBuf,

    /// PGDG `deb-src` sources file mounted into the build container.
    #[arg(long, default_value = "debian/pgdg-src.list")]
    pgdg_src_list: PathBuf,
}

impl Push {
    pub async fn run(self) -> Result<(), Error> {
        for target in self.selection.resolve(DEBIAN_ONLY)? {
            crate::debian::push(
                &self.target_dir,
                &self.patches_dir,
                &self.pgdg_src_list,
                debian_package(target),
            )
            .await?;
        }
        Ok(())
    }
}

#[derive(Debug, clap::Parser)]
pub struct ListPatches {
    #[arg(long, default_value = "patches")]
    patches_dir: PathBuf,
}

impl ListPatches {
    pub async fn run(self) -> Result<(), Error> {
        let entries = crate::patches::list(&self.patches_dir)?;
        if entries.is_empty() {
            println!("No patches found in {}", self.patches_dir.display());
            return Ok(());
        }

        let target_width = entries
            .iter()
            .map(|entry| entry.target.as_os_str().len())
            .max()
            .unwrap_or(0)
            .max("TARGET".len());
        let footgun_width = entries
            .iter()
            .map(|entry| entry.footgun.len())
            .max()
            .unwrap_or(0)
            .max("FOOTGUN".len());

        println!(
            "{:<target_width$}  {:<footgun_width$}  PATCH",
            "TARGET", "FOOTGUN"
        );
        for entry in &entries {
            println!(
                "{:<target_width$}  {:<footgun_width$}  {}",
                entry.target.display(),
                entry.footgun,
                entry.path.display()
            );
        }

        Ok(())
    }
}
