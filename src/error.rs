use miette::Diagnostic;
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
pub enum CommandError {
    #[error(transparent)]
    #[diagnostic(code(io_error))]
    IoError(#[from] std::io::Error),

    #[error("Repository not initialized.")]
    #[diagnostic(code(restic::not_initialized))]
    NotInitialized,

    #[error("Restic exited with errors {0}.")]
    #[diagnostic(code(restic::error))]
    ResticError(String),

    #[error("Unexpected response from restic.")]
    #[diagnostic(code(restic::invalid_json))]
    InvalidResponse(#[from] serde_json::error::Error),

    #[error("Missing {0} configuration for job.")]
    #[diagnostic(code(restic::invalid_config))]
    MissingBackendConfig(&'static str),

    #[error("Required value for {0} not specific in the defaults or job specific configuration.")]
    #[diagnostic(code(restic::invalid_config))]
    MissingConfigValue(&'static str),
}

impl PartialEq for CommandError {
    fn eq(&self, other: &Self) -> bool {
        core::mem::discriminant(self) == core::mem::discriminant(other)
    }
}

pub type ComRes<T> = std::result::Result<T, CommandError>;
