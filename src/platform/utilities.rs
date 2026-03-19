use crate::config::model::AppConfig;
use crate::platform::locator::{Locator, PlatformVersion, UtilityLocation, UtilityType};
use crate::platform::process::{ProcessExecutor, ProcessRunner};
use tracing::info;

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
        info!(utility = ?utility, "locating platform utility");
        let location = self.locator.locate(utility)?;
        info!(utility = ?utility, path = %location.path.display(), "platform utility resolved");
        Ok(location)
    }

    /// Return the standard runner path for the requested utility.
    ///
    /// This stage always returns the standard non-interactive executor. Future EDT work may add a
    /// different path for `UtilityType::EdtCli` without changing call sites that go through this
    /// facade.
    pub fn runner_for(&self, _utility: UtilityType) -> &dyn ProcessRunner {
        &self.standard_runner
    }

    #[cfg(test)]
    fn with_locator(locator: Locator) -> Self {
        Self {
            locator,
            standard_runner: ProcessExecutor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PlatformUtilities;
    use crate::platform::locator::{Locator, LocatorError, UtilityType};
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    #[cfg(unix)]
    #[test]
    fn locate_edt_cli_uses_configured_binary_path() {
        let dir = tempdir().expect("tempdir");
        let binary = dir.path().join("1cedtcli");
        fs::write(&binary, "#!/bin/sh\nexit 0\n").expect("write");
        make_executable(&binary);
        let locator = Locator::with_roots(None, None, Some(binary.clone()), vec![], vec![]);
        let mut utilities = PlatformUtilities::with_locator(locator);

        let location = utilities.locate(UtilityType::EdtCli).expect("locate edt");

        assert_eq!(location.path, binary);
    }

    #[cfg(unix)]
    #[test]
    fn locate_edt_cli_returns_not_found_when_unconfigured() {
        let locator = Locator::with_roots(None, None, None, vec![], vec![]);
        let mut utilities = PlatformUtilities::with_locator(locator);

        let error = utilities
            .locate(UtilityType::EdtCli)
            .expect_err("expected not found");

        assert!(matches!(error, LocatorError::NotFound(UtilityType::EdtCli)));
    }
}
