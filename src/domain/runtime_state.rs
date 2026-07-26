use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::config::model::{BuilderBackend, InfobaseConfig, SourceFormat, SourceSetPurpose};
use crate::support::connection_args::split_v8_arg_string;
use crate::support::path::nearest_existing_canonical_path;

const STATE_NAMESPACE: &str = "v8-runner/runtime-state/v1";

/// Durable identity tying one source publication to its exact runtime-state commit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DumpTransactionId(String);

impl DumpTransactionId {
    pub(crate) fn new() -> Self {
        Self(uuid::Uuid::new_v4().hyphenated().to_string())
    }

    pub(crate) fn from_u128(value: u128) -> Self {
        Self(uuid::Uuid::from_u128(value).hyphenated().to_string())
    }

    pub(crate) fn as_u128(&self) -> u128 {
        uuid::Uuid::parse_str(&self.0)
            .expect("DumpTransactionId invariant")
            .as_u128()
    }
}

impl fmt::Display for DumpTransactionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for DumpTransactionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DumpTransactionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let uuid = uuid::Uuid::parse_str(&value).map_err(serde::de::Error::custom)?;
        let canonical = uuid.hyphenated().to_string();
        if canonical != value {
            return Err(serde::de::Error::custom(
                "dump transaction id must be a canonical UUID",
            ));
        }
        Ok(Self(canonical))
    }
}

/// A normalized, secret-free identity of one target infobase.
#[derive(Clone, PartialEq, Eq)]
pub struct InfobaseIdentity {
    connection_fingerprint: String,
    fingerprint: String,
    kind: InfobaseKind,
}

impl InfobaseIdentity {
    /// Normalize all supported 1C connection forms and exclude authentication material.
    pub fn normalize(config: &InfobaseConfig) -> Result<Self, RuntimeStateError> {
        let connection = NormalizedConnection::parse(&config.connection)?;
        let connection_fingerprint = tagged_hash(&connection.fields());
        let mut identity_fields = vec![
            ("namespace", STATE_NAMESPACE.as_bytes().to_vec()),
            ("connection", connection_fingerprint.as_bytes().to_vec()),
        ];
        if let Some(dbms) = &config.dbms {
            for (tag, value) in [
                ("dbms-kind", dbms.kind.as_deref()),
                ("dbms-server", dbms.server.as_deref()),
                ("dbms-name", dbms.name.as_deref()),
            ] {
                if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
                    identity_fields.push((tag, value.to_ascii_lowercase().into_bytes()));
                }
            }
        }

        Ok(Self {
            connection_fingerprint,
            fingerprint: tagged_hash(&identity_fields),
            kind: connection.kind(),
        })
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    #[cfg(test)]
    pub fn connection_fingerprint(&self) -> &str {
        &self.connection_fingerprint
    }
}

impl fmt::Debug for InfobaseIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InfobaseIdentity")
            .field("kind", &self.kind)
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InfobaseKind {
    File,
    Server,
}

enum NormalizedConnection {
    File(PathBuf),
    Server(Vec<(String, String)>),
}

impl NormalizedConnection {
    fn parse(raw: &str) -> Result<Self, RuntimeStateError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(RuntimeStateError::EmptyConnection);
        }
        if trimmed.starts_with('/') || trimmed.starts_with('-') {
            return Self::parse_raw(trimmed);
        }
        Self::parse_connection_string(trimmed)
    }

    fn parse_raw(raw: &str) -> Result<Self, RuntimeStateError> {
        let tokens = split_raw_args(raw)?;
        let mut connection: Option<Self> = None;
        let mut index = 0;
        while index < tokens.len() {
            let flag = tokens[index].as_str();
            if is_auth_flag(flag) {
                index += 1;
                if index >= tokens.len() {
                    return Err(RuntimeStateError::MalformedRawConnection);
                }
                index += 1;
                continue;
            }
            if connection.is_some() || index + 1 >= tokens.len() {
                return Err(RuntimeStateError::UnsupportedRawConnection);
            }
            let value = &tokens[index + 1];
            connection = Some(
                if flag.eq_ignore_ascii_case("/f") || flag.eq_ignore_ascii_case("-f") {
                    Self::File(canonicalize_stable(Path::new(value))?)
                } else if flag.eq_ignore_ascii_case("/s") || flag.eq_ignore_ascii_case("-s") {
                    let (server, reference) = value
                        .split_once('\\')
                        .ok_or(RuntimeStateError::MalformedRawConnection)?;
                    if server.trim().is_empty() || reference.trim().is_empty() {
                        return Err(RuntimeStateError::MalformedRawConnection);
                    }
                    Self::Server(vec![
                        ("ref".to_owned(), reference.trim().to_ascii_lowercase()),
                        ("srvr".to_owned(), server.trim().to_ascii_lowercase()),
                    ])
                } else if flag.eq_ignore_ascii_case("/ibconnectionstring")
                    || flag.eq_ignore_ascii_case("-ibconnectionstring")
                {
                    Self::parse_connection_string(value)?
                } else {
                    return Err(RuntimeStateError::UnsupportedRawConnection);
                },
            );
            index += 2;
        }
        connection.ok_or(RuntimeStateError::MalformedRawConnection)
    }

    fn parse_connection_string(raw: &str) -> Result<Self, RuntimeStateError> {
        let mut fields = Vec::new();
        let mut file_path = None;
        for item in split_connection_fields(raw)? {
            let (key, value) = item
                .split_once('=')
                .ok_or(RuntimeStateError::MalformedConnectionString)?;
            let key = key.trim().to_ascii_lowercase();
            if is_credential_key(&key) {
                continue;
            }
            let value = value.trim();
            if key == "file" {
                file_path = Some(canonicalize_stable(Path::new(value))?);
                continue;
            }
            fields.push((key, value.to_ascii_lowercase()));
        }
        if let Some(file_path) = file_path {
            return Ok(Self::File(file_path));
        }
        if fields.is_empty() {
            return Err(RuntimeStateError::MalformedConnectionString);
        }
        fields.sort();
        Ok(Self::Server(fields))
    }

    fn kind(&self) -> InfobaseKind {
        match self {
            Self::File(_) => InfobaseKind::File,
            Self::Server(_) => InfobaseKind::Server,
        }
    }

    fn fields(&self) -> Vec<(&'static str, Vec<u8>)> {
        match self {
            Self::File(path) => vec![("kind", b"file".to_vec()), ("path", path_bytes(path))],
            Self::Server(fields) => {
                let mut result = vec![("kind", b"server".to_vec())];
                for (key, value) in fields {
                    result.push(("field-key", key.as_bytes().to_vec()));
                    result.push(("field-value", value.as_bytes().to_vec()));
                }
                result
            }
        }
    }
}

/// Logical role separates configured, generated and tool-owned views of one source-set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalSourceRole {
    DesignerSource,
    EdtSource,
    ToolExtension,
}

impl LogicalSourceRole {
    fn label(self) -> &'static str {
        match self {
            Self::DesignerSource => "designer-source",
            Self::EdtSource => "edt-source",
            Self::ToolExtension => "tool-extension",
        }
    }
}

/// Identity inputs for one logical source view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSourceDescriptor {
    fingerprint: String,
}

/// Named inputs prevent format/backend/purpose fields from being accidentally swapped.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeSourceIdentityInputs<'a> {
    pub configured_source_identity: &'a Path,
    pub source_root: &'a Path,
    pub purpose: SourceSetPurpose,
    pub format: SourceFormat,
    pub backend: BuilderBackend,
    pub logical_role: LogicalSourceRole,
}

impl RuntimeSourceDescriptor {
    pub fn new(inputs: RuntimeSourceIdentityInputs<'_>) -> Result<Self, RuntimeStateError> {
        let canonical_root = canonicalize_stable(inputs.source_root)?;
        let fields = vec![
            (
                "configured-source",
                path_bytes(inputs.configured_source_identity),
            ),
            ("canonical-root", path_bytes(&canonical_root)),
            ("purpose", purpose_label(inputs.purpose).as_bytes().to_vec()),
            ("format", format_label(inputs.format).as_bytes().to_vec()),
            ("backend", backend_label(inputs.backend).as_bytes().to_vec()),
            (
                "logical-role",
                inputs.logical_role.label().as_bytes().to_vec(),
            ),
        ];
        Ok(Self {
            fingerprint: tagged_hash(&fields),
        })
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// Versioned root for all state associated with one infobase.
#[derive(Debug, Clone)]
pub struct RuntimeStateLayout {
    ib_state_dir: PathBuf,
}

impl RuntimeStateLayout {
    pub fn new(
        work_path: impl AsRef<Path>,
        identity: InfobaseIdentity,
    ) -> Result<Self, RuntimeStateError> {
        let work_path = canonicalize_stable(work_path.as_ref())?;
        let infobase_fingerprint = identity.fingerprint().to_owned();
        Ok(Self {
            ib_state_dir: work_path
                .join("ib-state")
                .join("v1")
                .join(&infobase_fingerprint),
        })
    }

    pub fn source_state(
        &self,
        source_set: &str,
        descriptor: &RuntimeSourceDescriptor,
    ) -> RuntimeSourceState {
        let safe_name = sanitize_source_set(source_set);
        let context_fingerprint = tagged_hash(&[
            ("source-set-name", source_set.as_bytes().to_vec()),
            (
                "source-identity",
                descriptor.fingerprint().as_bytes().to_vec(),
            ),
        ]);
        RuntimeSourceState {
            state_dir: self
                .ib_state_dir
                .join(format!("{safe_name}-{context_fingerprint}")),
            context_fingerprint,
        }
    }
}

/// Paths owned by one source view inside one infobase namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSourceState {
    state_dir: PathBuf,
    context_fingerprint: String,
}

impl RuntimeSourceState {
    #[allow(dead_code)] // Foundation consumed by the private CDFI/baseline tasks in this plan.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn hash_storage_path(&self) -> PathBuf {
        self.state_dir.join("hash-storage.redb")
    }

    #[cfg(test)]
    pub fn context_fingerprint(&self) -> &str {
        &self.context_fingerprint
    }

    #[allow(dead_code)] // Foundation consumed by the private Designer transaction task.
    pub fn private_cdfi_path(&self) -> PathBuf {
        self.state_dir.join("ConfigDumpInfo.xml")
    }

    #[allow(dead_code)] // Foundation consumed by the recoverable publication task.
    pub fn transactions_dir(&self) -> PathBuf {
        self.state_dir.join("transactions")
    }

    #[allow(dead_code)] // Typed handle intentionally lands before its persistence implementation.
    pub fn baseline(&self, role: BaselineRole, generation: StateGeneration) -> IbBaseline {
        IbBaseline {
            path: self
                .state_dir
                .join("generations")
                .join(generation.value.to_string())
                .join("ib-baseline")
                .join(role.label()),
            generation,
            role,
        }
    }

    #[allow(dead_code)]
    pub fn ib_baseline(&self, generation: StateGeneration) -> IbBaseline {
        self.baseline(BaselineRole::ConfiguredSource, generation)
    }

    #[allow(dead_code)] // Typed handle intentionally lands before its persistence implementation.
    pub fn source_observation(&self, generation: StateGeneration) -> SourceObservation {
        SourceObservation {
            path: self
                .state_dir
                .join("generations")
                .join(generation.value.to_string())
                .join("source-observation"),
            generation,
        }
    }
}

/// Optimistic generation shared by the typed runtime-state handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Foundation consumed by later runtime-state generation commits.
pub struct StateGeneration {
    value: u64,
}

/// Semantic origin of a complete private baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineRole {
    /// The platform dump corresponding to the configured source tree.
    ConfiguredSource,
    /// The intermediate Designer tree produced for an EDT source set.
    EdtPlatformDesigner,
}

impl BaselineRole {
    const fn label(self) -> &'static str {
        match self {
            Self::ConfiguredSource => "configured-source",
            Self::EdtPlatformDesigner => "edt-platform-designer",
        }
    }
}

impl StateGeneration {
    #[allow(dead_code)]
    pub const fn new(value: u64) -> Self {
        Self { value }
    }

    #[allow(dead_code)]
    pub const fn value(self) -> u64 {
        self.value
    }
}

/// Complete private infobase baseline for one state generation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Distinct type prevents baseline/observation path mix-ups in later tasks.
pub struct IbBaseline {
    path: PathBuf,
    generation: StateGeneration,
    role: BaselineRole,
}

impl IbBaseline {
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[allow(dead_code)]
    pub const fn generation(&self) -> StateGeneration {
        self.generation
    }

    #[cfg(test)]
    pub const fn role(&self) -> BaselineRole {
        self.role
    }
}

/// Last successfully observed source snapshot for one state generation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Distinct type prevents baseline/observation path mix-ups in later tasks.
pub struct SourceObservation {
    path: PathBuf,
    generation: StateGeneration,
}

impl SourceObservation {
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[allow(dead_code)]
    pub const fn generation(&self) -> StateGeneration {
        self.generation
    }
}

#[derive(Debug, Error)]
pub enum RuntimeStateError {
    #[error("infobase connection is empty")]
    EmptyConnection,
    #[error("infobase connection string is malformed")]
    MalformedConnectionString,
    #[error("raw infobase connection is malformed")]
    MalformedRawConnection,
    #[error("raw infobase connection form is unsupported")]
    UnsupportedRawConnection,
    #[error("runtime identity path cannot be resolved: {0}")]
    PathResolution(#[from] std::io::Error),
}

fn split_raw_args(raw: &str) -> Result<Vec<String>, RuntimeStateError> {
    let (tokens, balanced_quotes) = split_v8_arg_string(raw);
    if !balanced_quotes {
        return Err(RuntimeStateError::MalformedRawConnection);
    }
    Ok(tokens)
}

fn is_auth_flag(value: &str) -> bool {
    ["/n", "-n", "/p", "-p"]
        .iter()
        .any(|flag| value.eq_ignore_ascii_case(flag))
}

fn is_credential_key(value: &str) -> bool {
    matches!(
        value,
        "usr"
            | "user"
            | "username"
            | "pwd"
            | "password"
            | "dbuser"
            | "dbusr"
            | "dbpassword"
            | "dbpwd"
    )
}

fn split_connection_fields(raw: &str) -> Result<Vec<String>, RuntimeStateError> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut characters = raw.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' if quoted && characters.peek() == Some(&'"') => {
                characters.next();
                current.push('"');
            }
            '"' => quoted = !quoted,
            ';' if !quoted => {
                let field = current.trim();
                if !field.is_empty() {
                    fields.push(field.to_owned());
                }
                current.clear();
            }
            value => current.push(value),
        }
    }
    if quoted {
        return Err(RuntimeStateError::MalformedConnectionString);
    }
    let field = current.trim();
    if !field.is_empty() {
        fields.push(field.to_owned());
    }
    Ok(fields)
}

fn canonicalize_stable(path: &Path) -> Result<PathBuf, RuntimeStateError> {
    nearest_existing_canonical_path(path).map_err(RuntimeStateError::from)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

/// Windows paths are hashed as deterministic little-endian UTF-16 code units.
/// Case is preserved: the runtime layout does not guess filesystem case semantics.
#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn tagged_hash(fields: &[(&str, Vec<u8>)]) -> String {
    let mut hasher = Sha256::new();
    for (tag, value) in fields {
        hasher.update((tag.len() as u64).to_be_bytes());
        hasher.update(tag.as_bytes());
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value);
    }
    format!("{:x}", hasher.finalize())
}

fn purpose_label(value: SourceSetPurpose) -> &'static str {
    match value {
        SourceSetPurpose::Configuration => "configuration",
        SourceSetPurpose::Extension => "extension",
        SourceSetPurpose::ExternalDataProcessors => "external-data-processors",
        SourceSetPurpose::ExternalReports => "external-reports",
    }
}

fn format_label(value: SourceFormat) -> &'static str {
    match value {
        SourceFormat::Designer => "designer",
        SourceFormat::Edt => "edt",
    }
}

fn backend_label(value: BuilderBackend) -> &'static str {
    match value {
        BuilderBackend::Designer => "designer",
        BuilderBackend::Ibcmd => "ibcmd",
    }
}

fn sanitize_source_set(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches(['.', '_', '-']);
    if sanitized.is_empty() {
        "source-set".to_owned()
    } else {
        sanitized.chars().take(64).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BaselineRole, InfobaseIdentity, LogicalSourceRole, RuntimeSourceDescriptor,
        RuntimeSourceIdentityInputs, RuntimeStateLayout, StateGeneration,
    };
    use crate::config::model::{
        BuilderBackend, InfobaseConfig, InfobaseDbmsConfig, SourceFormat, SourceSetPurpose,
    };
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn file_infobase(connection: impl Into<String>) -> InfobaseConfig {
        InfobaseConfig {
            connection: connection.into(),
            user: None,
            password: None,
            dbms: None,
        }
    }

    #[test]
    fn equivalent_plain_and_raw_file_connections_have_one_identity() {
        let dir = tempdir().expect("tempdir");
        let ib = dir.path().join("ib");
        std::fs::create_dir(&ib).expect("ib");
        let plain = file_infobase(format!("File={}", ib.display()));
        let raw_slash = file_infobase(format!("/F \"{}\"", ib.display()));
        let raw_dash = file_infobase(format!("-F {}", ib.display()));
        let wrapped = file_infobase(format!("/IBConnectionString File={}", ib.display()));

        let expected = InfobaseIdentity::normalize(&plain).expect("plain identity");
        assert_eq!(
            expected.fingerprint(),
            InfobaseIdentity::normalize(&raw_slash)
                .expect("slash identity")
                .fingerprint()
        );
        assert_eq!(
            expected.fingerprint(),
            InfobaseIdentity::normalize(&raw_dash)
                .expect("dash identity")
                .fingerprint()
        );
        assert_eq!(
            expected.fingerprint(),
            InfobaseIdentity::normalize(&wrapped)
                .expect("wrapped identity")
                .fingerprint()
        );
    }

    #[test]
    fn quoted_semicolon_file_path_is_not_split_as_a_field_separator() {
        let dir = tempdir().expect("tempdir");
        let semicolon_path = dir.path().join("ib;accounting");
        let plain = file_infobase(format!("File=\"{}\"", semicolon_path.display()));
        let raw = file_infobase(format!("/F \"{}\"", semicolon_path.display()));
        let different = file_infobase(format!("File={}", dir.path().join("ib").display()));

        let plain = InfobaseIdentity::normalize(&plain).expect("quoted identity");
        assert_eq!(
            plain.fingerprint(),
            InfobaseIdentity::normalize(&raw)
                .expect("raw identity")
                .fingerprint()
        );
        assert_ne!(
            plain.fingerprint(),
            InfobaseIdentity::normalize(&different)
                .expect("different identity")
                .fingerprint()
        );
    }

    #[test]
    fn doubled_quotes_are_accepted_inside_plain_connection_values() {
        InfobaseIdentity::normalize(&file_infobase("File=\"ib\"\"quoted\""))
            .expect("doubled quote");
    }

    #[test]
    fn raw_windows_path_with_trailing_separator_uses_execution_quote_semantics() {
        let raw = file_infobase(r#"/F "C:\work\ib\""#);
        let plain = file_infobase(r#"File="C:\work\ib\""#);

        assert_eq!(
            InfobaseIdentity::normalize(&raw)
                .expect("raw identity")
                .fingerprint(),
            InfobaseIdentity::normalize(&plain)
                .expect("plain identity")
                .fingerprint()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_nonexistent_path_suffix_preserves_case_in_identity() {
        let dir = tempdir().expect("tempdir");
        let upper = file_infobase(format!("File={}\\FutureIB", dir.path().display()));
        let lower = file_infobase(format!("File={}\\futureib", dir.path().display()));

        assert_ne!(
            InfobaseIdentity::normalize(&upper)
                .expect("upper")
                .fingerprint(),
            InfobaseIdentity::normalize(&lower)
                .expect("lower")
                .fingerprint()
        );
    }

    #[test]
    fn malformed_quotes_and_trailing_fields_are_rejected() {
        for connection in [
            "File=\"/tmp/unterminated",
            r#"File="ib\"quoted""#,
            "File=/tmp/ib;broken",
            "/F /tmp/ib /N",
            "/F /tmp/ib /P",
        ] {
            assert!(
                InfobaseIdentity::normalize(&file_infobase(connection)).is_err(),
                "accepted malformed connection: {connection}"
            );
        }
    }

    #[test]
    fn equivalent_plain_and_raw_server_connections_have_one_identity() {
        let plain = file_infobase("Srvr=Demo;Ref=Accounting");
        let raw = file_infobase("/S Demo\\Accounting");

        assert_eq!(
            InfobaseIdentity::normalize(&plain)
                .expect("plain identity")
                .fingerprint(),
            InfobaseIdentity::normalize(&raw)
                .expect("raw identity")
                .fingerprint()
        );
    }

    #[test]
    fn nearest_existing_parent_makes_nonexistent_paths_stable() {
        let dir = tempdir().expect("tempdir");
        let existing = dir.path().join("existing");
        std::fs::create_dir(&existing).expect("existing");
        let through_dot_segments = existing.join("missing").join("..").join("future-ib");
        let direct = existing.join("future-ib");

        assert_eq!(
            InfobaseIdentity::normalize(&file_infobase(format!(
                "File={}",
                through_dot_segments.display()
            )))
            .expect("first identity")
            .fingerprint(),
            InfobaseIdentity::normalize(&file_infobase(format!("File={}", direct.display())))
                .expect("second identity")
                .fingerprint()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_file_paths_resolve_to_the_same_identity() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let real = dir.path().join("real");
        let link = dir.path().join("link");
        std::fs::create_dir(&real).expect("real");
        symlink(&real, &link).expect("symlink");

        assert_eq!(
            InfobaseIdentity::normalize(&file_infobase(format!("File={}", real.display())))
                .expect("real identity")
                .fingerprint(),
            InfobaseIdentity::normalize(&file_infobase(format!("File={}", link.display())))
                .expect("link identity")
                .fingerprint()
        );
    }

    #[test]
    fn credentials_do_not_affect_identity_or_debug_output() {
        let clean = file_infobase("Srvr=demo;Ref=accounting");
        let with_secrets = InfobaseConfig {
            connection: "Srvr=demo;Ref=accounting;Usr=alice;Pwd=hunter2;DBUser=sa;DBPwd=db-secret"
                .to_owned(),
            user: Some("top-secret-user".to_owned()),
            password: Some("top-secret-password".to_owned()),
            dbms: Some(
                InfobaseDbmsConfig::new("PostgreSQL", "db", "accounting")
                    .with_credentials(Some("db-admin".to_owned()), Some("db-password".to_owned())),
            ),
        };
        let clean = InfobaseIdentity::normalize(&clean).expect("clean identity");
        let secret = InfobaseIdentity::normalize(&with_secrets).expect("secret identity");
        let debug = format!("{secret:?}");

        assert_eq!(
            clean.connection_fingerprint(),
            secret.connection_fingerprint()
        );
        for value in [
            "alice",
            "hunter2",
            "sa",
            "db-secret",
            "top-secret-user",
            "top-secret-password",
            "db-admin",
            "db-password",
            "Srvr=demo",
        ] {
            assert!(!debug.contains(value), "Debug leaked {value:?}: {debug}");
        }
    }

    #[test]
    fn all_auth_sources_are_excluded_from_the_final_identity() {
        let dbms_a = InfobaseDbmsConfig::new("PostgreSQL", "db", "accounting")
            .with_credentials(Some("db-user-a".to_owned()), Some("db-pass-a".to_owned()));
        let dbms_b = InfobaseDbmsConfig::new("PostgreSQL", "db", "accounting")
            .with_credentials(Some("db-user-b".to_owned()), Some("db-pass-b".to_owned()));
        let plain = InfobaseConfig {
            connection: "Srvr=demo;Ref=accounting;Usr=plain-user;Pwd=plain-pass".to_owned(),
            user: Some("config-user-a".to_owned()),
            password: Some("config-pass-a".to_owned()),
            dbms: Some(dbms_a),
        };
        let raw = InfobaseConfig {
            connection: "/S demo\\accounting /N raw-user /P raw-pass".to_owned(),
            user: Some("config-user-b".to_owned()),
            password: Some("config-pass-b".to_owned()),
            dbms: Some(dbms_b),
        };

        assert_eq!(
            InfobaseIdentity::normalize(&plain)
                .expect("plain identity")
                .fingerprint(),
            InfobaseIdentity::normalize(&raw)
                .expect("raw identity")
                .fingerprint()
        );
    }

    #[test]
    fn unsupported_raw_connection_is_rejected_without_echoing_it() {
        let error = InfobaseIdentity::normalize(&file_infobase("/Unknown secret-value"))
            .expect_err("unsupported raw form");

        assert!(!error.to_string().contains("secret-value"));
    }

    #[test]
    fn distinct_infobases_and_dbms_targets_have_distinct_identities() {
        let dir = tempdir().expect("tempdir");
        let first = InfobaseIdentity::normalize(&file_infobase(format!(
            "File={}",
            dir.path().join("first").display()
        )))
        .expect("first");
        let second = InfobaseIdentity::normalize(&file_infobase(format!(
            "File={}",
            dir.path().join("second").display()
        )))
        .expect("second");
        assert_ne!(first.fingerprint(), second.fingerprint());

        let server_a = InfobaseConfig::server(
            "Srvr=cluster;Ref=accounting",
            InfobaseDbmsConfig::new("PostgreSQL", "db-a", "accounting"),
        );
        let server_b = InfobaseConfig::server(
            "Srvr=cluster;Ref=accounting",
            InfobaseDbmsConfig::new("PostgreSQL", "db-b", "accounting"),
        );
        assert_ne!(
            InfobaseIdentity::normalize(&server_a)
                .expect("server a")
                .fingerprint(),
            InfobaseIdentity::normalize(&server_b)
                .expect("server b")
                .fingerprint()
        );
    }

    #[test]
    fn layout_is_versioned_and_context_identity_covers_every_contract_field() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        let source = dir.path().join("source");
        std::fs::create_dir(&work).expect("work");
        std::fs::create_dir(&source).expect("source");
        let identity = InfobaseIdentity::normalize(&file_infobase(format!(
            "File={}",
            dir.path().join("ib").display()
        )))
        .expect("identity");
        let ib_fingerprint = identity.fingerprint().to_owned();
        let layout = RuntimeStateLayout::new(&work, identity).expect("layout");
        let descriptor = RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
            configured_source_identity: Path::new("configured/source"),
            source_root: &source,
            purpose: SourceSetPurpose::Configuration,
            format: SourceFormat::Designer,
            backend: BuilderBackend::Designer,
            logical_role: LogicalSourceRole::DesignerSource,
        })
        .expect("descriptor");
        let state = layout.source_state("main", &descriptor);

        assert_eq!(
            state.state_dir(),
            std::fs::canonicalize(&work)
                .expect("canonical work")
                .join("ib-state")
                .join("v1")
                .join(ib_fingerprint)
                .join(format!("main-{}", state.context_fingerprint()))
        );
        assert_eq!(
            state.hash_storage_path(),
            state.state_dir().join("hash-storage.redb")
        );
        assert_eq!(
            state.private_cdfi_path(),
            state.state_dir().join("ConfigDumpInfo.xml")
        );
        assert_eq!(
            state.transactions_dir(),
            state.state_dir().join("transactions")
        );
        assert_eq!(
            state
                .ib_baseline(StateGeneration::new(7))
                .generation()
                .value(),
            7
        );
        assert_eq!(
            state
                .source_observation(StateGeneration::new(7))
                .generation()
                .value(),
            7
        );

        let variants = [
            RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
                configured_source_identity: Path::new("configured/other"),
                source_root: &source,
                purpose: SourceSetPurpose::Configuration,
                format: SourceFormat::Designer,
                backend: BuilderBackend::Designer,
                logical_role: LogicalSourceRole::DesignerSource,
            })
            .expect("raw identity"),
            RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
                configured_source_identity: Path::new("configured/source"),
                source_root: &dir.path().join("other-root"),
                purpose: SourceSetPurpose::Configuration,
                format: SourceFormat::Designer,
                backend: BuilderBackend::Designer,
                logical_role: LogicalSourceRole::DesignerSource,
            })
            .expect("root identity"),
            RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
                configured_source_identity: Path::new("configured/source"),
                source_root: &source,
                purpose: SourceSetPurpose::Extension,
                format: SourceFormat::Designer,
                backend: BuilderBackend::Designer,
                logical_role: LogicalSourceRole::DesignerSource,
            })
            .expect("purpose identity"),
            RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
                configured_source_identity: Path::new("configured/source"),
                source_root: &source,
                purpose: SourceSetPurpose::Configuration,
                format: SourceFormat::Edt,
                backend: BuilderBackend::Designer,
                logical_role: LogicalSourceRole::DesignerSource,
            })
            .expect("format identity"),
            RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
                configured_source_identity: Path::new("configured/source"),
                source_root: &source,
                purpose: SourceSetPurpose::Configuration,
                format: SourceFormat::Designer,
                backend: BuilderBackend::Ibcmd,
                logical_role: LogicalSourceRole::DesignerSource,
            })
            .expect("backend identity"),
            RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
                configured_source_identity: Path::new("configured/source"),
                source_root: &source,
                purpose: SourceSetPurpose::Configuration,
                format: SourceFormat::Designer,
                backend: BuilderBackend::Designer,
                logical_role: LogicalSourceRole::EdtSource,
            })
            .expect("role identity"),
        ];
        for variant in variants {
            assert_ne!(descriptor.fingerprint(), variant.fingerprint());
        }
    }

    #[test]
    fn source_set_name_is_sanitized_to_one_safe_segment() {
        let dir = tempdir().expect("tempdir");
        let layout = RuntimeStateLayout::new(
            dir.path().join("work"),
            InfobaseIdentity::normalize(&file_infobase(format!(
                "File={}",
                dir.path().join("ib").display()
            )))
            .expect("identity"),
        )
        .expect("layout");
        let descriptor = RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
            configured_source_identity: Path::new("source"),
            source_root: &dir.path().join("source"),
            purpose: SourceSetPurpose::Configuration,
            format: SourceFormat::Designer,
            backend: BuilderBackend::Designer,
            logical_role: LogicalSourceRole::DesignerSource,
        })
        .expect("descriptor");

        let state = layout.source_state("../unsafe/name", &descriptor);
        let segment = state
            .state_dir()
            .file_name()
            .and_then(|name| name.to_str())
            .expect("state segment");
        assert!(!segment.contains('/'));
        assert!(!segment.contains(".."));
    }

    #[test]
    fn sanitized_source_set_collisions_still_have_distinct_context_fingerprints() {
        let dir = tempdir().expect("tempdir");
        let layout = RuntimeStateLayout::new(
            dir.path().join("work"),
            InfobaseIdentity::normalize(&file_infobase(format!(
                "File={}",
                dir.path().join("ib").display()
            )))
            .expect("identity"),
        )
        .expect("layout");
        let descriptor = RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
            configured_source_identity: Path::new("source"),
            source_root: &dir.path().join("source"),
            purpose: SourceSetPurpose::Configuration,
            format: SourceFormat::Designer,
            backend: BuilderBackend::Designer,
            logical_role: LogicalSourceRole::DesignerSource,
        })
        .expect("descriptor");

        let slash = layout.source_state("unsafe/name", &descriptor);
        let backslash = layout.source_state("unsafe\\name", &descriptor);

        assert_ne!(slash.state_dir(), backslash.state_dir());
    }

    #[cfg(unix)]
    #[test]
    fn canonical_path_preserves_symlink_parent_semantics() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let real = dir.path().join("real");
        let child = real.join("child");
        let target = real.join("target");
        let link = dir.path().join("link");
        std::fs::create_dir_all(&child).expect("child");
        std::fs::create_dir(&target).expect("target");
        symlink(&child, &link).expect("link");

        assert_eq!(
            InfobaseIdentity::normalize(&file_infobase(format!(
                "File={}",
                link.join("..").join("target").display()
            )))
            .expect("symlink parent identity")
            .fingerprint(),
            InfobaseIdentity::normalize(&file_infobase(format!("File={}", target.display())))
                .expect("target identity")
                .fingerprint()
        );
    }

    #[test]
    fn baseline_and_observation_are_distinct_generation_bound_handles() {
        fn baseline_path(value: &super::IbBaseline) -> PathBuf {
            value.path().to_path_buf()
        }
        fn observation_path(value: &super::SourceObservation) -> PathBuf {
            value.path().to_path_buf()
        }

        let dir = tempdir().expect("tempdir");
        let layout = RuntimeStateLayout::new(
            dir.path().join("work"),
            InfobaseIdentity::normalize(&file_infobase(format!(
                "File={}",
                dir.path().join("ib").display()
            )))
            .expect("identity"),
        )
        .expect("layout");
        let descriptor = RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
            configured_source_identity: Path::new("source"),
            source_root: &dir.path().join("source"),
            purpose: SourceSetPurpose::Configuration,
            format: SourceFormat::Designer,
            backend: BuilderBackend::Designer,
            logical_role: LogicalSourceRole::DesignerSource,
        })
        .expect("descriptor");
        let state = layout.source_state("main", &descriptor);
        let baseline = state.ib_baseline(StateGeneration::new(3));
        let observation = state.source_observation(StateGeneration::new(3));

        assert_ne!(baseline_path(&baseline), observation_path(&observation));
        assert!(baseline.path().starts_with(state.state_dir()));
        assert!(observation.path().starts_with(state.state_dir()));
    }

    #[test]
    fn baseline_roles_are_distinct_within_one_generation() {
        let dir = tempdir().expect("tempdir");
        let layout = RuntimeStateLayout::new(
            dir.path().join("work"),
            InfobaseIdentity::normalize(&file_infobase(format!(
                "File={}",
                dir.path().join("ib").display()
            )))
            .expect("identity"),
        )
        .expect("layout");
        let descriptor = RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
            configured_source_identity: Path::new("source"),
            source_root: &dir.path().join("source"),
            purpose: SourceSetPurpose::Configuration,
            format: SourceFormat::Edt,
            backend: BuilderBackend::Designer,
            logical_role: LogicalSourceRole::EdtSource,
        })
        .expect("descriptor");
        let state = layout.source_state("main", &descriptor);
        let generation = StateGeneration::new(5);
        let configured = state.baseline(BaselineRole::ConfiguredSource, generation);
        let platform = state.baseline(BaselineRole::EdtPlatformDesigner, generation);

        assert_eq!(configured.role(), BaselineRole::ConfiguredSource);
        assert_eq!(platform.role(), BaselineRole::EdtPlatformDesigner);
        assert_ne!(configured.path(), platform.path());
        assert_eq!(configured.generation(), platform.generation());
    }
}
