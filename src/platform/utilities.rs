use crate::config::model::AppConfig;
use crate::platform::locator::{Locator, PlatformVersion, UtilityLocation, UtilityType};
use crate::platform::process::{ProcessExecutor, ProcessRunner};

/// Facade over utility discovery and standard executor selection.
pub struct PlatformUtilities {
    locator: Locator,
    standard_runner: ProcessExecutor,
}

impl PlatformUtilities {
    /// Build platform utilities facade from application configuration.
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            locator: Locator::new(
                config.tools.platform.path.clone(),
                config
                    .tools
                    .platform
                    .version
                    .as_deref()
                    .and_then(PlatformVersion::parse_strict),
                config.tools.edt_cli.path.clone(),
            ),
            standard_runner: ProcessExecutor,
        }
    }

    /// Resolve an executable for the requested utility.
    pub fn locate(
        &mut self,
        utility: UtilityType,
    ) -> Result<UtilityLocation, crate::platform::locator::LocatorError> {
        self.locator.locate(utility)
    }

    /// Return the standard runner path for the requested utility.
    ///
    /// This stage always returns the standard non-interactive executor. Future EDT work may add a
    /// different path for `UtilityType::EdtCli` without changing call sites that go through this
    /// facade.
    pub fn runner_for(&self, _utility: UtilityType) -> &dyn ProcessRunner {
        &self.standard_runner
    }
}
