#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Git(#[from] git_proc::Error),

    #[error(transparent)]
    PlatformNotAccepted(#[from] crate::target::PlatformNotAccepted),

    #[error("no patches found in {0}")]
    NoPatches(std::path::PathBuf),

    #[error(transparent)]
    Backend(#[from] ociman::backend::resolve::Error),

    #[error(transparent)]
    Container(#[from] ociman::WithContainerError),

    #[error(transparent)]
    Push(#[from] ociman::backend::PushError),

    #[error(transparent)]
    Command(#[from] cmd_proc::CommandError),

    #[error("failed to {action} {path}")]
    Io {
        action: &'static str,
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("unpacked source for {package} not found under {path}")]
    SourceNotUnpacked {
        package: String,
        path: std::path::PathBuf,
    },

    #[error("no {package} server .deb found under {path}")]
    NoServerDeb {
        package: String,
        path: std::path::PathBuf,
    },
}
