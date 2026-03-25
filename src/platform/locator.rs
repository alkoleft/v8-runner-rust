use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Executable-oriented platform utility identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UtilityType {
    /// `1cv8`
    V8,
    /// `1cv8c`
    V8C,
    /// `ibcmd`
    Ibcmd,
    /// `1cedtcli`
    EdtCli,
}

impl UtilityType {
    /// Executable filename for the current platform.
    pub fn executable_name(self) -> &'static str {
        match self {
            Self::V8 => executable_name_for("1cv8"),
            Self::V8C => executable_name_for("1cv8c"),
            Self::Ibcmd => executable_name_for("ibcmd"),
            Self::EdtCli => executable_name_for("1cedtcli"),
        }
    }

    /// Returns `true` for regular 1C platform binaries.
    pub fn is_platform(self) -> bool {
        !matches!(self, Self::EdtCli)
    }
}

impl fmt::Display for UtilityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.executable_name())
    }
}

/// Exact 4-part 1C platform version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlatformVersion {
    /// Major version component.
    pub major: u32,
    /// Minor version component.
    pub minor: u32,
    /// Patch version component.
    pub patch: u32,
    /// Build version component.
    pub build: u32,
}

impl PlatformVersion {
    /// Parse a strict `major.minor.patch.build` string.
    pub fn parse_strict(value: &str) -> Option<Self> {
        let parts = value
            .split('.')
            .map(str::trim)
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;

        if parts.len() != 4 {
            return None;
        }

        Some(Self {
            major: parts[0],
            minor: parts[1],
            patch: parts[2],
            build: parts[3],
        })
    }
}

impl fmt::Display for PlatformVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}",
            self.major, self.minor, self.patch, self.build
        )
    }
}

/// EDT discovery version parsed leniently from numeric tokens.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdtVersion {
    /// Numeric tokens extracted from a path or directory name.
    pub parts: Vec<u32>,
}

impl EdtVersion {
    /// Parse a version from any string that contains numeric tokens.
    pub fn parse_lenient(value: &str) -> Option<Self> {
        let parts: Vec<u32> = value
            .split(|ch: char| !ch.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse::<u32>().ok())
            .collect();

        if parts.is_empty() {
            None
        } else {
            Some(Self { parts })
        }
    }
}

/// Parsed utility version metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UtilityVersion {
    /// Exact 1C platform version.
    Platform(PlatformVersion),
    /// Lenient EDT discovery version.
    Edt(EdtVersion),
}

/// Resolved utility path together with parsed version information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtilityLocation {
    /// Utility kind that was resolved.
    pub utility: UtilityType,
    /// Absolute path to the executable.
    pub path: PathBuf,
    /// Parsed version metadata if it could be derived from the path.
    pub version: Option<UtilityVersion>,
}

#[derive(Debug, Error)]
pub enum LocatorError {
    #[error("utility '{0}' was not found")]
    NotFound(UtilityType),
}

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    version: Option<UtilityVersion>,
}

/// Stateful utility locator with per-instance cache.
pub struct Locator {
    platform_hint: Option<PathBuf>,
    platform_version: Option<PlatformVersion>,
    edt_hint: Option<PathBuf>,
    edt_version: Option<EdtVersion>,
    cache: HashMap<(UtilityType, Option<String>), UtilityLocation>,
    platform_roots: Vec<PathBuf>,
    edt_roots: Vec<PathBuf>,
}

impl Locator {
    /// Build a locator using default OS-specific search roots.
    pub fn new(
        platform_hint: Option<PathBuf>,
        platform_version: Option<PlatformVersion>,
        edt_hint: Option<PathBuf>,
        edt_version: Option<EdtVersion>,
    ) -> Self {
        Self {
            platform_hint,
            platform_version,
            edt_hint,
            edt_version,
            cache: HashMap::new(),
            platform_roots: default_platform_roots(),
            edt_roots: default_edt_roots(),
        }
    }

    /// Resolve an executable path for the requested utility.
    pub fn locate(&mut self, utility: UtilityType) -> Result<UtilityLocation, LocatorError> {
        let cache_key = (utility, self.version_requirement_string(utility));

        if let Some(cached) = self.cache.get(&cache_key).cloned() {
            if is_valid_executable(&cached.path) {
                return Ok(cached);
            }
            self.cache.remove(&cache_key);
        }

        if let Some(location) = self.resolve_explicit_hint(utility) {
            self.cache.insert(cache_key, location.clone());
            return Ok(location);
        }

        let hint_candidates = self.search_hint_candidates(utility);
        if !hint_candidates.is_empty() {
            let selected = self.select_candidate(utility, hint_candidates)?;
            self.cache.insert(cache_key, selected.clone());
            return Ok(selected);
        }

        let candidates = self.search_candidates(utility);
        let selected = self.select_candidate(utility, candidates)?;
        self.cache.insert(cache_key, selected.clone());
        Ok(selected)
    }

    #[cfg(test)]
    pub(crate) fn with_roots(
        platform_hint: Option<PathBuf>,
        platform_version: Option<PlatformVersion>,
        edt_hint: Option<PathBuf>,
        edt_version: Option<EdtVersion>,
        platform_roots: Vec<PathBuf>,
        edt_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            platform_hint,
            platform_version,
            edt_hint,
            edt_version,
            cache: HashMap::new(),
            platform_roots,
            edt_roots,
        }
    }

    fn version_requirement_string(&self, utility: UtilityType) -> Option<String> {
        if utility.is_platform() {
            self.platform_version.as_ref().map(ToString::to_string)
        } else {
            self.edt_version.as_ref().map(|version| {
                version
                    .parts
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(".")
            })
        }
    }

    fn resolve_explicit_hint(&self, utility: UtilityType) -> Option<UtilityLocation> {
        let hint = if utility.is_platform() {
            self.platform_hint.as_deref()
        } else {
            self.edt_hint.as_deref()
        }?;

        let candidate = resolve_from_hint(hint, utility)?;
        if !is_valid_executable(&candidate) {
            return None;
        }

        Some(UtilityLocation {
            utility,
            version: infer_version(utility, &candidate),
            path: candidate,
        })
    }

    fn search_candidates(&self, utility: UtilityType) -> Vec<Candidate> {
        let mut candidates = Vec::new();

        if utility.is_platform() {
            if let Some(required) = &self.platform_version {
                candidates.extend(platform_candidates_for_version(
                    utility,
                    required,
                    &self.platform_roots,
                ));
            } else {
                candidates.extend(platform_candidates_any_version(
                    utility,
                    &self.platform_roots,
                ));
            }
        } else {
            if let Some(required) = &self.edt_version {
                candidates.extend(edt_candidates_for_version(
                    utility,
                    required,
                    &self.edt_roots,
                ));
            } else {
                candidates.extend(edt_candidates_any_version(utility, &self.edt_roots));
            }
        }

        candidates.extend(path_candidates(utility));
        candidates
    }

    fn search_hint_candidates(&self, utility: UtilityType) -> Vec<Candidate> {
        if utility.is_platform() {
            let Some(hint) = self.platform_hint.as_ref() else {
                return Vec::new();
            };
            if !hint.is_dir() {
                return Vec::new();
            }

            if let Some(required) = &self.platform_version {
                platform_candidates_for_version(utility, required, std::slice::from_ref(hint))
            } else {
                platform_candidates_any_version(utility, std::slice::from_ref(hint))
            }
        } else {
            let Some(hint) = self.edt_hint.as_ref() else {
                return Vec::new();
            };
            if !hint.is_dir() {
                return Vec::new();
            }

            if let Some(required) = &self.edt_version {
                edt_candidates_for_version(utility, required, std::slice::from_ref(hint))
            } else {
                edt_candidates_any_version(utility, std::slice::from_ref(hint))
            }
        }
    }

    fn select_candidate(
        &self,
        utility: UtilityType,
        mut candidates: Vec<Candidate>,
    ) -> Result<UtilityLocation, LocatorError> {
        candidates.retain(|candidate| is_valid_executable(&candidate.path));

        if candidates.is_empty() {
            return Err(LocatorError::NotFound(utility));
        }

        let chosen = if utility.is_platform() {
            if let Some(required) = &self.platform_version {
                candidates
                    .into_iter()
                    .find(|candidate| {
                        matches!(
                            candidate.version.as_ref(),
                            Some(UtilityVersion::Platform(version)) if version == required
                        )
                    })
                    .ok_or(LocatorError::NotFound(utility))?
            } else {
                candidates
                    .into_iter()
                    .max_by(|left, right| {
                        compare_versions(left.version.as_ref(), right.version.as_ref())
                    })
                    .ok_or(LocatorError::NotFound(utility))?
            }
        } else {
            candidates
                .into_iter()
                .max_by(|left, right| {
                    compare_versions(left.version.as_ref(), right.version.as_ref())
                })
                .ok_or(LocatorError::NotFound(utility))?
        };

        Ok(UtilityLocation {
            utility,
            path: chosen.path,
            version: chosen.version,
        })
    }
}

fn compare_versions(
    left: Option<&UtilityVersion>,
    right: Option<&UtilityVersion>,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(UtilityVersion::Platform(a)), Some(UtilityVersion::Platform(b))) => a.cmp(b),
        (Some(UtilityVersion::Edt(a)), Some(UtilityVersion::Edt(b))) => a.cmp(b),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        _ => std::cmp::Ordering::Equal,
    }
}

fn resolve_from_hint(hint: &Path, utility: UtilityType) -> Option<PathBuf> {
    if hint.is_file() {
        let target_name = utility.executable_name();
        let file_name_matches = hint
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name == target_name)
            .unwrap_or(false);

        return if file_name_matches {
            Some(hint.to_path_buf())
        } else {
            hint.parent().map(|parent| parent.join(target_name))
        };
    }

    if hint.is_dir() {
        let direct = hint.join(utility.executable_name());
        if direct.exists() {
            return Some(direct);
        }
        return Some(hint.join("bin").join(utility.executable_name()));
    }

    None
}

fn platform_candidates_for_version(
    utility: UtilityType,
    version: &PlatformVersion,
    roots: &[PathBuf],
) -> Vec<Candidate> {
    let version_dir = version.to_string();
    roots
        .iter()
        .flat_map(|root| {
            [
                root.join(&version_dir).join(utility.executable_name()),
                root.join(&version_dir)
                    .join("bin")
                    .join(utility.executable_name()),
            ]
        })
        .map(|path| Candidate {
            path,
            version: Some(UtilityVersion::Platform(version.clone())),
        })
        .collect()
}

fn platform_candidates_any_version(utility: UtilityType, roots: &[PathBuf]) -> Vec<Candidate> {
    let mut candidates = Vec::new();

    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let Some(dir_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(version) = PlatformVersion::parse_strict(dir_name) else {
                continue;
            };

            candidates.push(Candidate {
                path: path.join(utility.executable_name()),
                version: Some(UtilityVersion::Platform(version.clone())),
            });
            candidates.push(Candidate {
                path: path.join("bin").join(utility.executable_name()),
                version: Some(UtilityVersion::Platform(version)),
            });
        }
    }

    candidates
}

fn edt_candidates_any_version(utility: UtilityType, roots: &[PathBuf]) -> Vec<Candidate> {
    let mut candidates = Vec::new();

    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let version = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(EdtVersion::parse_lenient)
                .map(UtilityVersion::Edt);

            candidates.push(Candidate {
                path: path.join("1cedt").join(utility.executable_name()),
                version: version.clone(),
            });
            candidates.push(Candidate {
                path: path.join(utility.executable_name()),
                version,
            });
        }
    }

    candidates
}

fn edt_candidates_for_version(
    utility: UtilityType,
    required: &EdtVersion,
    roots: &[PathBuf],
) -> Vec<Candidate> {
    edt_candidates_any_version(utility, roots)
        .into_iter()
        .filter(|candidate| {
            matches!(
                candidate.version.as_ref(),
                Some(UtilityVersion::Edt(version)) if edt_version_matches(required, version)
            )
        })
        .collect()
}

fn edt_version_matches(required: &EdtVersion, candidate: &EdtVersion) -> bool {
    if required.parts.is_empty() {
        return false;
    }

    candidate
        .parts
        .windows(required.parts.len())
        .any(|window| window == required.parts.as_slice())
}

fn path_candidates(utility: UtilityType) -> Vec<Candidate> {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| {
                    let path = dir.join(utility.executable_name());
                    Candidate {
                        version: infer_version(utility, &path),
                        path,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn infer_version(utility: UtilityType, path: &Path) -> Option<UtilityVersion> {
    let component_strings: Vec<String> = path
        .ancestors()
        .flat_map(|ancestor| {
            ancestor
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .collect();

    if utility.is_platform() {
        component_strings
            .iter()
            .find_map(|value| PlatformVersion::parse_strict(value))
            .map(UtilityVersion::Platform)
    } else {
        component_strings
            .iter()
            .find_map(|value| EdtVersion::parse_lenient(value))
            .map(UtilityVersion::Edt)
    }
}

fn is_valid_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn executable_name_for(base: &'static str) -> &'static str {
    #[cfg(windows)]
    {
        match base {
            "1cv8" => "1cv8.exe",
            "1cv8c" => "1cv8c.exe",
            "ibcmd" => "ibcmd.exe",
            "1cedtcli" => "1cedtcli.exe",
            _ => base,
        }
    }

    #[cfg(not(windows))]
    {
        base
    }
}

fn default_platform_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        vec![
            PathBuf::from(r"C:\Program Files\1cv8"),
            PathBuf::from(r"C:\Program Files (x86)\1cv8"),
        ]
    }

    #[cfg(target_os = "linux")]
    {
        vec![
            PathBuf::from("/opt/1cv8/x86_64"),
            PathBuf::from("/opt/1cv8/i386"),
            PathBuf::from("/usr/local/1cv8"),
        ]
    }

    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        vec![PathBuf::from("/opt/1cv8")]
    }
}

fn default_edt_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        vec![PathBuf::from(r"C:\Program Files\1C\1CE\components")]
    }

    #[cfg(target_os = "linux")]
    {
        vec![PathBuf::from("/opt/1C/1CE/components")]
    }

    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        vec![PathBuf::from("/opt/1C/1CE/components")]
    }
}

#[cfg(test)]
mod tests {
    use super::{EdtVersion, Locator, PlatformVersion, UtilityType, UtilityVersion};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    #[cfg(unix)]
    fn touch_executable(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(path, "#!/bin/sh\nexit 0\n").expect("write");
        make_executable(path);
    }

    #[test]
    fn parse_strict_platform_version_requires_four_parts() {
        assert!(PlatformVersion::parse_strict("8.3.25").is_none());
        assert!(PlatformVersion::parse_strict("8.3.25.1234").is_some());
    }

    #[test]
    fn parse_lenient_edt_version_extracts_numeric_tokens() {
        let version = EdtVersion::parse_lenient("1c-edt-2025.1.0+656-x86_64").expect("version");
        assert_eq!(version.parts, vec![1, 2025, 1, 0, 656, 86, 64]);
    }

    #[cfg(unix)]
    #[test]
    fn explicit_file_hint_supports_sibling_binary_lookup() {
        let dir = tempdir().expect("tempdir");
        let v8 = dir.path().join("1cv8");
        let v8c = dir.path().join("1cv8c");
        touch_executable(&v8);
        touch_executable(&v8c);

        let mut locator = Locator::with_roots(Some(v8.clone()), None, None, None, vec![], vec![]);

        assert_eq!(locator.locate(UtilityType::V8C).expect("locate").path, v8c);
    }

    #[cfg(unix)]
    #[test]
    fn explicit_directory_hint_checks_direct_and_bin_layouts() {
        let dir = tempdir().expect("tempdir");
        let install_dir = dir.path().join("install");
        let binary = install_dir.join("bin").join("1cv8");
        touch_executable(&binary);

        let mut locator = Locator::with_roots(Some(install_dir), None, None, None, vec![], vec![]);

        assert_eq!(
            locator.locate(UtilityType::V8).expect("locate").path,
            binary
        );
    }

    #[cfg(unix)]
    #[test]
    fn explicit_root_hint_searches_versioned_children() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("platform-root");
        let version = PlatformVersion::parse_strict("8.3.25.1234").expect("version");
        let thin = root.join("8.3.25.1234").join("bin").join("1cv8c");
        touch_executable(&thin);

        let mut locator = Locator::with_roots(Some(root), Some(version), None, None, vec![], vec![]);

        assert_eq!(locator.locate(UtilityType::V8C).expect("locate").path, thin);
    }

    #[cfg(unix)]
    #[test]
    fn platform_search_prefers_exact_version_match() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("platform");
        let wanted = root.join("8.3.25.1234").join("1cv8");
        let other = root.join("8.3.24.9999").join("1cv8");
        touch_executable(&wanted);
        touch_executable(&other);

        let mut locator = Locator::with_roots(
            None,
            Some(PlatformVersion::parse_strict("8.3.25.1234").expect("version")),
            None,
            None,
            vec![root],
            vec![],
        );

        let location = locator.locate(UtilityType::V8).expect("locate");
        assert_eq!(location.path, wanted);
        assert!(matches!(
            location.version,
            Some(UtilityVersion::Platform(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn edt_search_picks_highest_lenient_version() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("edt");
        let newer = root
            .join("1c-edt-2025.1.0+656-x86_64")
            .join("1cedt")
            .join("1cedtcli");
        let older = root
            .join("1c-edt-2024.2.0+100-x86_64")
            .join("1cedt")
            .join("1cedtcli");
        touch_executable(&newer);
        touch_executable(&older);

        let mut locator = Locator::with_roots(None, None, None, None, vec![], vec![root]);

        assert_eq!(
            locator.locate(UtilityType::EdtCli).expect("locate").path,
            newer
        );
    }

    #[cfg(unix)]
    #[test]
    fn invalidates_broken_cache_entries_and_relocates() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("platform");
        let version = PlatformVersion::parse_strict("8.3.25.1234").expect("version");
        let first = root.join("8.3.25.1234").join("1cv8");
        touch_executable(&first);

        let mut locator =
            Locator::with_roots(None, Some(version), None, None, vec![root.clone()], vec![]);
        let first_path = locator.locate(UtilityType::V8).expect("first").path;
        assert_eq!(first_path, first);

        fs::remove_file(&first).expect("remove");
        let second = root.join("8.3.25.1234").join("bin").join("1cv8");
        touch_executable(&second);

        let second_path = locator.locate(UtilityType::V8).expect("second").path;
        assert_eq!(second_path, second);
    }

    #[test]
    fn utility_location_can_infer_platform_version_from_path() {
        let path = PathBuf::from("/opt/1cv8/x86_64/8.3.25.1234/1cv8");
        let version = super::infer_version(UtilityType::V8, &path);

        assert!(matches!(version, Some(UtilityVersion::Platform(_))));
    }

    #[cfg(unix)]
    #[test]
    fn edt_search_accepts_version_prefix_hint() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("edt");
        let wanted = root
            .join("1c-edt-2025.2.3+30-x86_64")
            .join("1cedt")
            .join("1cedtcli");
        let other = root
            .join("1c-edt-2025.1.9+100-x86_64")
            .join("1cedt")
            .join("1cedtcli");
        touch_executable(&wanted);
        touch_executable(&other);

        let mut locator = Locator::with_roots(
            None,
            None,
            None,
            Some(EdtVersion::parse_lenient("1c-edt-2025.2.3").expect("version")),
            vec![],
            vec![root],
        );

        assert_eq!(
            locator.locate(UtilityType::EdtCli).expect("locate").path,
            wanted
        );
    }

    #[cfg(unix)]
    #[test]
    fn edt_search_accepts_plain_numeric_version_hint() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("edt");
        let wanted = root
            .join("1c-edt-2025.2.3+30-x86_64")
            .join("1cedt")
            .join("1cedtcli");
        let other = root
            .join("1c-edt-2025.1.9+100-x86_64")
            .join("1cedt")
            .join("1cedtcli");
        touch_executable(&wanted);
        touch_executable(&other);

        let mut locator = Locator::with_roots(
            None,
            None,
            None,
            Some(EdtVersion::parse_lenient("2025.2.3").expect("version")),
            vec![],
            vec![root],
        );

        assert_eq!(
            locator.locate(UtilityType::EdtCli).expect("locate").path,
            wanted
        );
    }
}
