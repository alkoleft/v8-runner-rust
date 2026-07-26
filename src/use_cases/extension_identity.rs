use crate::config::model::{SourceSetConfig, SourceSetPurpose};

/// Extension name passed to platform commands (`-Extension` / `--name`).
///
/// The configured source-set name is the extension identity for both Designer and EDT.
/// EDT `.project` name is a workspace/project identity and must not override it.
pub fn platform_extension_name(source_set: &SourceSetConfig) -> &str {
    debug_assert_eq!(source_set.purpose, SourceSetPurpose::Extension);
    source_set.name.as_str()
}

#[cfg(test)]
mod tests {
    use super::platform_extension_name;
    use crate::config::model::{SourceSetConfig, SourceSetPurpose};
    use std::path::PathBuf;

    #[test]
    fn platform_extension_name_uses_source_set_name() {
        let source_set = SourceSetConfig {
            name: "SalesAddon".to_owned(),
            purpose: SourceSetPurpose::Extension,
            path: PathBuf::from("extensions/sales-project"),
            depends_on: Vec::new(),
        };

        assert_eq!(platform_extension_name(&source_set), "SalesAddon");
    }
}
