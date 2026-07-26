use crate::config::loader::ConfigLoadError;
use crate::domain::runtime_state::RuntimeStateError;
use crate::platform::designer::DesignerError;
use crate::platform::edt::EdtError;
use crate::platform::edt_session::EdtSessionError;
use crate::platform::enterprise::EnterpriseError;
use crate::platform::ibcmd::IbcmdError;
use crate::platform::locator::LocatorError;
use crate::platform::process::ProcessError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("validation error: {0}")]
    Validation(String),

    #[error("validation error: {0}")]
    ValidationIbcmd(#[source] IbcmdError),

    #[error("validation error: {context}; {source}")]
    ValidationIbcmdContext {
        context: String,
        #[source]
        source: IbcmdError,
    },

    #[error("runtime error: {0}")]
    Runtime(String),

    #[error("platform error: {0}")]
    Platform(String),

    #[error("platform error: {0}")]
    PlatformDesigner(#[source] DesignerError),

    #[error("platform error: {context}; {source}")]
    PlatformDesignerContext {
        context: String,
        #[source]
        source: DesignerError,
    },

    #[error("platform error: {0}")]
    PlatformLocator(#[from] LocatorError),

    #[error("platform error: {0}")]
    PlatformProcess(#[from] ProcessError),

    #[error("platform error: {context}; {source}")]
    PlatformLocatorContext {
        context: String,
        #[source]
        source: LocatorError,
    },

    #[error("platform error: {context}; {source}")]
    PlatformProcessContext {
        context: String,
        #[source]
        source: ProcessError,
    },

    #[error("platform error: {0}")]
    PlatformEdt(#[source] EdtError),

    #[error("platform error: {context}; {source}")]
    PlatformEdtContext {
        context: String,
        #[source]
        source: EdtError,
    },

    #[error("platform error: {0}")]
    PlatformEdtSession(#[source] EdtSessionError),

    #[error("platform error: {context}; {source}")]
    PlatformEdtSessionContext {
        context: String,
        #[source]
        source: EdtSessionError,
    },

    #[error(transparent)]
    Config(#[from] ConfigLoadError),

    #[error("validation error: {context}; {source}")]
    ConfigContext {
        context: String,
        #[source]
        source: ConfigLoadError,
    },
}

impl AppError {
    pub fn with_context(self, context: impl Into<String>) -> Self {
        let context = context.into();
        match self {
            Self::Validation(message) => Self::Validation(format!("{context}; {message}")),
            Self::ValidationIbcmd(source) => Self::ValidationIbcmdContext { context, source },
            Self::ValidationIbcmdContext {
                context: existing,
                source,
            } => Self::ValidationIbcmdContext {
                context: format!("{context}; {existing}"),
                source,
            },
            Self::Runtime(message) => Self::Runtime(format!("{context}; {message}")),
            Self::Platform(message) => Self::Platform(format!("{context}; {message}")),
            Self::PlatformDesigner(source) => Self::PlatformDesignerContext { context, source },
            Self::PlatformDesignerContext {
                context: existing,
                source,
            } => Self::PlatformDesignerContext {
                context: format!("{context}; {existing}"),
                source,
            },
            Self::PlatformLocator(source) => Self::PlatformLocatorContext { context, source },
            Self::PlatformProcess(source) => Self::PlatformProcessContext { context, source },
            Self::PlatformEdt(source) => Self::PlatformEdtContext { context, source },
            Self::PlatformEdtSession(source) => Self::PlatformEdtSessionContext { context, source },
            Self::PlatformLocatorContext {
                context: existing,
                source,
            } => Self::PlatformLocatorContext {
                context: format!("{context}; {existing}"),
                source,
            },
            Self::PlatformProcessContext {
                context: existing,
                source,
            } => Self::PlatformProcessContext {
                context: format!("{context}; {existing}"),
                source,
            },
            Self::PlatformEdtContext {
                context: existing,
                source,
            } => Self::PlatformEdtContext {
                context: format!("{context}; {existing}"),
                source,
            },
            Self::PlatformEdtSessionContext {
                context: existing,
                source,
            } => Self::PlatformEdtSessionContext {
                context: format!("{context}; {existing}"),
                source,
            },
            Self::Config(source) => Self::ConfigContext { context, source },
            Self::ConfigContext {
                context: existing,
                source,
            } => Self::ConfigContext {
                context: format!("{context}; {existing}"),
                source,
            },
        }
    }
}

impl From<RuntimeStateError> for AppError {
    fn from(error: RuntimeStateError) -> Self {
        match error {
            RuntimeStateError::PathResolution(_) => Self::Runtime(error.to_string()),
            RuntimeStateError::EmptyConnection
            | RuntimeStateError::MalformedConnectionString
            | RuntimeStateError::MalformedRawConnection
            | RuntimeStateError::UnsupportedRawConnection => Self::Validation(error.to_string()),
        }
    }
}

impl From<IbcmdError> for AppError {
    fn from(error: IbcmdError) -> Self {
        match error {
            IbcmdError::MissingServerDbmsField(_) => Self::ValidationIbcmd(error),
            IbcmdError::Spawn(error) => Self::PlatformProcess(error),
        }
    }
}

impl From<DesignerError> for AppError {
    fn from(error: DesignerError) -> Self {
        match error {
            DesignerError::UtilityNotFound(_) => Self::PlatformDesigner(error),
            DesignerError::Spawn(error) => Self::PlatformProcess(error),
        }
    }
}

impl From<EdtError> for AppError {
    fn from(error: EdtError) -> Self {
        match error {
            EdtError::Spawn(error) => Self::PlatformProcess(error),
            error @ EdtError::PrepareWorkspace { .. } => Self::PlatformEdt(error),
            error @ EdtError::Interactive(_) => Self::PlatformEdt(error),
            error @ EdtError::SharedSession(_) => Self::PlatformEdt(error),
        }
    }
}

impl From<EdtSessionError> for AppError {
    fn from(error: EdtSessionError) -> Self {
        Self::PlatformEdtSession(error)
    }
}

impl From<EnterpriseError> for AppError {
    fn from(error: EnterpriseError) -> Self {
        match error {
            EnterpriseError::Spawn(error) => Self::PlatformProcess(error),
        }
    }
}
