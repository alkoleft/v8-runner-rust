use std::sync::Arc;

use crate::config::model::AppConfig;
use crate::domain::build::BuildResult;
use crate::domain::dump::DumpResult;
use crate::domain::launch::LaunchResult;
use crate::domain::syntax::SyntaxCheckResult;
use crate::domain::test::TestRunResult;
use crate::use_cases::build_project;
use crate::use_cases::check_syntax;
use crate::use_cases::context::ExecutionContext;
use crate::use_cases::dump_config;
use crate::use_cases::launch_app;
use crate::use_cases::request::{
    BuildRequest, DumpRequest, LaunchRequest, SyntaxRequest, TestRequest,
};
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use crate::use_cases::run_tests;
use crate::use_cases::transport::dispatch_with_workspace_lock;

/// Thin indirection layer used by the MCP service to call use cases.
pub trait McpUseCasePort {
    fn build_project(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &BuildRequest,
    ) -> UseCaseResult<BuildResult>;

    fn run_tests(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &TestRequest,
    ) -> UseCaseResult<TestRunResult>;

    fn dump_config(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &DumpRequest,
    ) -> UseCaseResult<DumpResult>;

    fn launch_app(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &LaunchRequest,
    ) -> UseCaseResult<LaunchResult>;

    fn check_syntax(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &SyntaxRequest,
    ) -> UseCaseResult<SyntaxCheckResult>;
}

/// Production port implementation delegating directly to use cases.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultMcpUseCasePort;

impl McpUseCasePort for DefaultMcpUseCasePort {
    fn build_project(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &BuildRequest,
    ) -> UseCaseResult<BuildResult> {
        with_workspace_lock(context, config, || {
            build_project::execute(context, config, request)
        })
    }

    fn run_tests(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &TestRequest,
    ) -> UseCaseResult<TestRunResult> {
        with_workspace_lock(context, config, || {
            run_tests::execute(context, config, request)
        })
    }

    fn dump_config(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &DumpRequest,
    ) -> UseCaseResult<DumpResult> {
        with_workspace_lock(context, config, || {
            dump_config::execute(context, config, request)
        })
    }

    fn launch_app(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &LaunchRequest,
    ) -> UseCaseResult<LaunchResult> {
        with_workspace_lock(context, config, || {
            launch_app::execute(context, config, request)
        })
    }

    fn check_syntax(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &SyntaxRequest,
    ) -> UseCaseResult<SyntaxCheckResult> {
        with_workspace_lock(context, config, || {
            check_syntax::execute(context, config, request)
        })
    }
}

fn with_workspace_lock<T>(
    context: &ExecutionContext,
    config: &AppConfig,
    run: impl FnOnce() -> UseCaseResult<T>,
) -> UseCaseResult<T> {
    match dispatch_with_workspace_lock(config, context.command(), || Ok(()), run) {
        Ok(result) => result,
        Err(error) => Err(UseCaseFailure::without_payload(error)),
    }
}

impl<T> McpUseCasePort for Arc<T>
where
    T: McpUseCasePort + ?Sized,
{
    fn build_project(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &BuildRequest,
    ) -> UseCaseResult<BuildResult> {
        (**self).build_project(context, config, request)
    }

    fn run_tests(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &TestRequest,
    ) -> UseCaseResult<TestRunResult> {
        (**self).run_tests(context, config, request)
    }

    fn dump_config(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &DumpRequest,
    ) -> UseCaseResult<DumpResult> {
        (**self).dump_config(context, config, request)
    }

    fn launch_app(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &LaunchRequest,
    ) -> UseCaseResult<LaunchResult> {
        (**self).launch_app(context, config, request)
    }

    fn check_syntax(
        &self,
        context: &ExecutionContext,
        config: &AppConfig,
        request: &SyntaxRequest,
    ) -> UseCaseResult<SyntaxCheckResult> {
        (**self).check_syntax(context, config, request)
    }
}

#[cfg(test)]
mod tests {
    use super::{DefaultMcpUseCasePort, McpUseCasePort};
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
        TestsConfig, ToolsConfig,
    };
    use crate::support::fs::acquire_advisory_lock;
    use crate::use_cases::context::{CommandName, ExecutionContext};
    use crate::use_cases::request::BuildRequest;
    use crate::use_cases::result::UseCaseErrorKind;
    use crate::use_cases::workspace_lock::workspace_lock_path;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn sample_config(work_path: &Path) -> AppConfig {
        AppConfig {
            base_path: work_path.join("base"),
            work_path: work_path.to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: PathBuf::from("main"),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    #[test]
    fn default_port_reports_workspace_lock_conflict_before_use_case_dispatch() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        fs::create_dir_all(&work).expect("work dir");
        let config = sample_config(&work);
        let canonical_work = fs::canonicalize(&config.work_path).expect("canonical work");
        let lock_path = workspace_lock_path(&canonical_work);
        let _guard = acquire_advisory_lock(&lock_path).expect("workspace lock");

        let failure = DefaultMcpUseCasePort
            .build_project(
                &ExecutionContext::mcp_stdio(CommandName::Build),
                &config,
                &BuildRequest {
                    full_rebuild: true,
                    source_set: None,
                    dynamic_update: None,
                },
            )
            .expect_err("busy workspace");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Runtime);
        assert!(failure.error.to_string().contains("workspace"));
        assert!(failure.error.to_string().contains("already"));
    }
}
