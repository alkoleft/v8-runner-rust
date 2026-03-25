use crate::config::model::AppConfig;
use crate::platform::locator::{EdtVersion, Locator, PlatformVersion, UtilityLocation, UtilityType};
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
        let edt_hint = config.tools.edt_cli.path.clone().filter(|path| {
            path.is_absolute()
                || path.components().count() > 1
                || path.exists()
                || config.tools.edt_cli.version.is_none()
        });
        let edt_version = config
            .tools
            .edt_cli
            .version
            .as_deref()
            .and_then(EdtVersion::parse_lenient)
            .or_else(|| {
                config
                    .tools
                    .edt_cli
                    .path
                    .as_ref()
                    .and_then(|path| path.to_str())
                    .filter(|value| !value.contains(std::path::MAIN_SEPARATOR))
                    .and_then(EdtVersion::parse_lenient)
            });
        Self {
            locator: Locator::new(
                config.tools.platform.path.clone(),
                config
                    .tools
                    .platform
                    .version
                    .as_deref()
                    .and_then(PlatformVersion::parse_strict),
                edt_hint,
                edt_version,
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
    use crate::platform::locator::{EdtVersion, Locator, LocatorError, UtilityType};
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
        let locator = Locator::with_roots(None, None, Some(binary.clone()), None, vec![], vec![]);
        let mut utilities = PlatformUtilities::with_locator(locator);

        let location = utilities.locate(UtilityType::EdtCli).expect("locate edt");

        assert_eq!(location.path, binary);
    }

    #[cfg(unix)]
    #[test]
    fn locate_edt_cli_returns_not_found_when_unconfigured() {
        let locator = Locator::with_roots(None, None, None, None, vec![], vec![]);
        let mut utilities = PlatformUtilities::with_locator(locator);

        let error = utilities
            .locate(UtilityType::EdtCli)
            .expect_err("expected not found");

        assert!(matches!(error, LocatorError::NotFound(UtilityType::EdtCli)));
    }

    #[cfg(unix)]
    #[test]
    fn locate_edt_cli_uses_version_filtered_autodiscovery() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("components");
        let wanted = root
            .join("1c-edt-2025.2.3+30-x86_64")
            .join("1cedt")
            .join("1cedtcli");
        let older = root
            .join("1c-edt-2025.1.9+10-x86_64")
            .join("1cedt")
            .join("1cedtcli");
        fs::create_dir_all(wanted.parent().expect("wanted parent")).expect("wanted dirs");
        fs::create_dir_all(older.parent().expect("older parent")).expect("older dirs");
        fs::write(&wanted, "#!/bin/sh\nexit 0\n").expect("wanted");
        fs::write(&older, "#!/bin/sh\nexit 0\n").expect("older");
        make_executable(&wanted);
        make_executable(&older);

        let locator = Locator::with_roots(
            None,
            None,
            None,
            Some(EdtVersion::parse_lenient("1c-edt-2025.2.3").expect("version")),
            vec![],
            vec![root],
        );
        let mut utilities = PlatformUtilities::with_locator(locator);

        let location = utilities.locate(UtilityType::EdtCli).expect("locate edt");

        assert_eq!(location.path, wanted);
    }
}
