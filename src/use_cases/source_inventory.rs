use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::PathBuf;

use crate::change_detection::analyzer::ContextAnalysis;
use crate::change_detection::source_sets::SourceSetsService;
use crate::config::model::{AppConfig, SourceSetConfig, SourceSetPurpose};
use crate::domain::source_set::SourceSetContext;
use crate::support::error::AppError;

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

    /// Resolves a pre-validated dependency graph in stable canonical order.
    pub(crate) fn dependency_ordered_source_sets(
        &self,
        selected_name: Option<&str>,
    ) -> Result<Vec<&'a SourceSetConfig>, AppError> {
        let canonical = self.ordered_source_sets();
        if canonical
            .iter()
            .all(|source_set| source_set.depends_on.is_empty())
        {
            return match selected_name {
                Some(name) => self
                    .source_set(name)
                    .map(|source_set| vec![source_set])
                    .ok_or_else(|| AppError::Validation(format!("unknown source-set '{name}'"))),
                None => Ok(canonical),
            };
        }

        let included = self.dependency_closure(selected_name, &canonical)?;
        stable_topological_order(&canonical, &included)
    }

    fn dependency_closure(
        &self,
        selected_name: Option<&str>,
        canonical: &[&'a SourceSetConfig],
    ) -> Result<HashSet<&'a str>, AppError> {
        let Some(selected_name) = selected_name else {
            return Ok(canonical
                .iter()
                .map(|source_set| source_set.name.as_str())
                .collect());
        };

        let selected = self
            .source_set(selected_name)
            .ok_or_else(|| AppError::Validation(format!("unknown source-set '{selected_name}'")))?;
        let mut included = HashSet::new();
        let mut pending = vec![selected];
        while let Some(source_set) = pending.pop() {
            if !included.insert(source_set.name.as_str()) {
                continue;
            }
            for dependency_name in &source_set.depends_on {
                let dependency = self.source_set(dependency_name).ok_or_else(|| {
                    AppError::Validation(format!(
                        "source-set '{}' depends on unknown source-set '{}'",
                        source_set.name, dependency_name
                    ))
                })?;
                pending.push(dependency);
            }
        }
        Ok(included)
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

fn stable_topological_order<'a>(
    canonical: &[&'a SourceSetConfig],
    included: &HashSet<&str>,
) -> Result<Vec<&'a SourceSetConfig>, AppError> {
    let canonical_index = canonical
        .iter()
        .enumerate()
        .map(|(index, source_set)| (source_set.name.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut indegree = HashMap::new();
    let mut dependents = HashMap::<&str, Vec<&str>>::new();

    for source_set in canonical
        .iter()
        .copied()
        .filter(|source_set| included.contains(source_set.name.as_str()))
    {
        let mut dependency_count = 0;
        for dependency in &source_set.depends_on {
            if !canonical_index.contains_key(dependency.as_str()) {
                return Err(AppError::Validation(format!(
                    "source-set '{}' depends on unknown source-set '{}'",
                    source_set.name, dependency
                )));
            }
            if !included.contains(dependency.as_str()) {
                continue;
            }
            dependency_count += 1;
            dependents
                .entry(dependency.as_str())
                .or_default()
                .push(source_set.name.as_str());
        }
        indegree.insert(source_set.name.as_str(), dependency_count);
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(name, degree)| {
            if *degree == 0 {
                canonical_index.get(name).copied().map(Reverse)
            } else {
                None
            }
        })
        .collect::<BinaryHeap<_>>();
    let mut ordered = Vec::with_capacity(included.len());

    while let Some(Reverse(index)) = ready.pop() {
        let source_set = canonical[index];
        ordered.push(source_set);

        for dependent in dependents
            .get(source_set.name.as_str())
            .into_iter()
            .flatten()
        {
            let Some(degree) = indegree.get_mut(dependent) else {
                continue;
            };
            if *degree == 0 {
                continue;
            }
            *degree -= 1;
            if *degree == 0 {
                if let Some(index) = canonical_index.get(dependent).copied() {
                    ready.push(Reverse(index));
                }
            }
        }
    }

    if ordered.len() != included.len() {
        return Err(AppError::Validation(
            "source-set dependency graph contains a cycle".to_owned(),
        ));
    }
    Ok(ordered)
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
                    depends_on: Vec::new(),
                },
                SourceSetConfig {
                    name: "main".to_owned(),
                    purpose: SourceSetPurpose::Configuration,
                    path: "configuration".into(),
                    depends_on: Vec::new(),
                },
                SourceSetConfig {
                    name: "processors".to_owned(),
                    purpose: SourceSetPurpose::ExternalDataProcessors,
                    path: "external/processors".into(),
                    depends_on: Vec::new(),
                },
                SourceSetConfig {
                    name: "reports".to_owned(),
                    purpose: SourceSetPurpose::ExternalReports,
                    path: "external/reports".into(),
                    depends_on: Vec::new(),
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
    fn dependency_ordered_source_sets_use_canonical_order_for_ready_nodes() {
        let mut config = config(SourceFormat::Designer);
        config.source_sets = vec![
            SourceSetConfig {
                name: "tests".to_owned(),
                purpose: SourceSetPurpose::Extension,
                path: "extensions/tests".into(),
                depends_on: vec!["yaxunit".to_owned()],
            },
            SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: "configuration".into(),
                depends_on: Vec::new(),
            },
            SourceSetConfig {
                name: "unrelated".to_owned(),
                purpose: SourceSetPurpose::Extension,
                path: "extensions/unrelated".into(),
                depends_on: vec!["main".to_owned()],
            },
            SourceSetConfig {
                name: "yaxunit".to_owned(),
                purpose: SourceSetPurpose::Extension,
                path: "extensions/yaxunit".into(),
                depends_on: vec!["main".to_owned()],
            },
            SourceSetConfig {
                name: "processors".to_owned(),
                purpose: SourceSetPurpose::ExternalDataProcessors,
                path: "external/processors".into(),
                depends_on: Vec::new(),
            },
            SourceSetConfig {
                name: "reports".to_owned(),
                purpose: SourceSetPurpose::ExternalReports,
                path: "external/reports".into(),
                depends_on: Vec::new(),
            },
        ];
        let inventory = SourceSetInventory::new(&config);

        let names = inventory
            .dependency_ordered_source_sets(None)
            .expect("valid dependency graph")
            .into_iter()
            .map(|source_set| source_set.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "main",
                "unrelated",
                "yaxunit",
                "tests",
                "processors",
                "reports"
            ]
        );
    }

    #[test]
    fn dependency_ordered_source_sets_expand_scoped_diamond_once() {
        let mut config = config(SourceFormat::Designer);
        config.source_sets = vec![
            SourceSetConfig {
                name: "tests".to_owned(),
                purpose: SourceSetPurpose::Extension,
                path: "extensions/tests".into(),
                depends_on: vec!["left".to_owned(), "right".to_owned()],
            },
            SourceSetConfig {
                name: "unrelated".to_owned(),
                purpose: SourceSetPurpose::Extension,
                path: "extensions/unrelated".into(),
                depends_on: vec!["main".to_owned()],
            },
            SourceSetConfig {
                name: "right".to_owned(),
                purpose: SourceSetPurpose::Extension,
                path: "extensions/right".into(),
                depends_on: vec!["common".to_owned()],
            },
            SourceSetConfig {
                name: "left".to_owned(),
                purpose: SourceSetPurpose::Extension,
                path: "extensions/left".into(),
                depends_on: vec!["common".to_owned()],
            },
            SourceSetConfig {
                name: "common".to_owned(),
                purpose: SourceSetPurpose::Extension,
                path: "extensions/common".into(),
                depends_on: vec!["main".to_owned()],
            },
            SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: "configuration".into(),
                depends_on: Vec::new(),
            },
        ];
        let inventory = SourceSetInventory::new(&config);
        let names = inventory
            .dependency_ordered_source_sets(Some("tests"))
            .expect("valid dependency graph")
            .into_iter()
            .map(|source_set| source_set.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["main", "common", "right", "left", "tests"]);
    }

    #[test]
    fn dependency_ordered_source_sets_keep_legacy_canonical_order() {
        let config = config(SourceFormat::Designer);
        let inventory = SourceSetInventory::new(&config);

        let names = inventory
            .dependency_ordered_source_sets(None)
            .expect("valid dependency graph")
            .into_iter()
            .map(|source_set| source_set.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["main", "ext", "processors", "reports"]);
    }

    #[test]
    fn dependency_ordered_source_sets_reject_unknown_selection() {
        let config = config(SourceFormat::Designer);
        let inventory = SourceSetInventory::new(&config);

        let error = inventory
            .dependency_ordered_source_sets(Some("missing"))
            .expect_err("unknown selection must fail");

        assert_eq!(
            error.to_string(),
            "validation error: unknown source-set 'missing'"
        );
    }

    #[test]
    fn dependency_ordered_source_sets_reject_unknown_dependency() {
        let mut config = config(SourceFormat::Designer);
        config.source_sets[0].depends_on = vec!["missing".to_owned()];
        let inventory = SourceSetInventory::new(&config);

        let error = inventory
            .dependency_ordered_source_sets(Some("ext"))
            .expect_err("unknown dependency must fail");

        assert_eq!(
            error.to_string(),
            "validation error: source-set 'ext' depends on unknown source-set 'missing'"
        );
    }

    #[test]
    fn dependency_ordered_source_sets_reject_cycle() {
        let mut config = config(SourceFormat::Designer);
        config.source_sets[0].depends_on = vec!["main".to_owned()];
        config.source_sets[1].depends_on = vec!["ext".to_owned()];
        let inventory = SourceSetInventory::new(&config);

        let error = inventory
            .dependency_ordered_source_sets(Some("ext"))
            .expect_err("cycle must fail");

        assert_eq!(
            error.to_string(),
            "validation error: source-set dependency graph contains a cycle"
        );
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
