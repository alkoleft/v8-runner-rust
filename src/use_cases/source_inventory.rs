use std::collections::HashMap;
use std::path::PathBuf;

use crate::change_detection::analyzer::ContextAnalysis;
use crate::change_detection::source_sets::SourceSetsService;
use crate::config::model::{AppConfig, SourceSetConfig, SourceSetPurpose};
use crate::domain::source_set::SourceSetContext;

/// Read-only runtime index for source-set orchestration.
pub(crate) struct SourceSetInventory<'a> {
    config: &'a AppConfig,
    source_sets_by_name: HashMap<&'a str, &'a SourceSetConfig>,
    designer_contexts: Vec<SourceSetContext>,
    designer_contexts_by_name: HashMap<String, SourceSetContext>,
    edt_contexts: Vec<SourceSetContext>,
    edt_contexts_by_name: HashMap<String, SourceSetContext>,
}

impl<'a> SourceSetInventory<'a> {
    pub(crate) fn new(config: &'a AppConfig) -> Self {
        let service = SourceSetsService::new(config);
        let designer_contexts = service.designer_contexts();
        let edt_contexts = service.edt_contexts();

        Self {
            config,
            source_sets_by_name: config
                .source_sets
                .iter()
                .map(|source_set| (source_set.name.as_str(), source_set))
                .collect(),
            designer_contexts_by_name: index_contexts(&designer_contexts),
            designer_contexts,
            edt_contexts_by_name: index_contexts(&edt_contexts),
            edt_contexts,
        }
    }

    pub(crate) fn source_sets(&self) -> Vec<&'a SourceSetConfig> {
        self.config.source_sets.iter().collect()
    }

    pub(crate) fn ordered_source_sets(&self) -> Vec<&'a SourceSetConfig> {
        let mut configuration = Vec::new();
        let mut extensions = Vec::new();
        let mut external_processors = Vec::new();
        let mut external_reports = Vec::new();

        for source_set in &self.config.source_sets {
            match source_set.purpose {
                SourceSetPurpose::Configuration => configuration.push(source_set),
                SourceSetPurpose::Extension => extensions.push(source_set),
                SourceSetPurpose::ExternalDataProcessors => external_processors.push(source_set),
                SourceSetPurpose::ExternalReports => external_reports.push(source_set),
            }
        }

        configuration.extend(extensions);
        configuration.extend(external_processors);
        configuration.extend(external_reports);
        configuration
    }

    pub(crate) fn source_set(&self, name: &str) -> Option<&'a SourceSetConfig> {
        self.source_sets_by_name.get(name).copied()
    }

    pub(crate) fn source_sets_with_purpose(
        &self,
        purpose: SourceSetPurpose,
    ) -> Vec<&'a SourceSetConfig> {
        self.config
            .source_sets
            .iter()
            .filter(|source_set| source_set.purpose == purpose)
            .collect()
    }

    pub(crate) fn source_path(&self, source_set: &SourceSetConfig) -> PathBuf {
        if source_set.path.is_absolute() {
            source_set.path.clone()
        } else {
            self.config.base_path.join(&source_set.path)
        }
    }

    pub(crate) fn designer_contexts(&self) -> &[SourceSetContext] {
        &self.designer_contexts
    }

    pub(crate) fn designer_context(&self, source_set_name: &str) -> Option<&SourceSetContext> {
        self.designer_contexts_by_name.get(source_set_name)
    }

    pub(crate) fn edt_contexts(&self) -> &[SourceSetContext] {
        &self.edt_contexts
    }

    pub(crate) fn edt_context(&self, source_set_name: &str) -> Option<&SourceSetContext> {
        self.edt_contexts_by_name.get(source_set_name)
    }

    pub(crate) fn has_edt_contexts(&self) -> bool {
        !self.edt_contexts.is_empty()
    }

    pub(crate) fn analyze_contexts(&self, contexts: &[SourceSetContext]) -> Vec<ContextAnalysis> {
        SourceSetsService::new(self.config).analyze_contexts(contexts)
    }
}

fn index_contexts(contexts: &[SourceSetContext]) -> HashMap<String, SourceSetContext> {
    contexts
        .iter()
        .cloned()
        .map(|context| (context.name().to_owned(), context))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::SourceSetInventory;
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, InfobaseConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };

    fn config(format: SourceFormat) -> AppConfig {
        let root = std::env::current_dir()
            .expect("current dir")
            .join("target/source-inventory-tests");
        AppConfig {
            base_path: root.join("base"),
            work_path: root.join("work"),
            execution_timeout: 300_000,
            format,
            builder: BuilderBackend::Designer,
            infobase: InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![
                SourceSetConfig {
                    name: "ext".to_owned(),
                    purpose: SourceSetPurpose::Extension,
                    path: "extensions/ext".into(),
                },
                SourceSetConfig {
                    name: "main".to_owned(),
                    purpose: SourceSetPurpose::Configuration,
                    path: "configuration".into(),
                },
                SourceSetConfig {
                    name: "processors".to_owned(),
                    purpose: SourceSetPurpose::ExternalDataProcessors,
                    path: "external/processors".into(),
                },
                SourceSetConfig {
                    name: "reports".to_owned(),
                    purpose: SourceSetPurpose::ExternalReports,
                    path: "external/reports".into(),
                },
            ],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    #[test]
    fn ordered_source_sets_group_configuration_extensions_and_external_sets() {
        let config = config(SourceFormat::Designer);
        let inventory = SourceSetInventory::new(&config);

        let names = inventory
            .ordered_source_sets()
            .into_iter()
            .map(|source_set| source_set.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["main", "ext", "processors", "reports"]);
    }

    #[test]
    fn indexes_designer_and_edt_contexts_by_source_set_identity() {
        let config = config(SourceFormat::Edt);
        let inventory = SourceSetInventory::new(&config);

        let main = inventory.source_set("main").expect("main source-set");
        assert_eq!(
            inventory.source_path(main),
            config.base_path.join("configuration")
        );
        assert_eq!(
            inventory.designer_context("main").expect("designer").path(),
            config.work_path.join("designer/main").as_path()
        );
        assert_eq!(
            inventory.edt_context("main").expect("edt").path(),
            config.base_path.join("configuration").as_path()
        );
    }
}
