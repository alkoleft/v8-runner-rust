use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "v8-test-runner", about = "1C:Enterprise test runner CLI")]
pub struct Cli {
    /// Path to YAML config file
    #[arg(long, global = true, env = "V8TR_CONFIG")]
    pub config: Option<String>,

    /// Output format
    #[arg(long, global = true, default_value = "text", value_parser = ["text", "json"])]
    pub output: String,

    /// Log level
    #[arg(long, global = true, default_value = "info",
          value_parser = ["error", "warn", "info", "debug", "trace"])]
    pub log_level: Option<String>,

    /// Clear log files before execution
    #[arg(long, global = true)]
    pub clean_before_execution: bool,

    /// Disable ANSI colors
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Override working directory
    #[arg(long, global = true)]
    pub workdir: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialize infobase and EDT workspace
    Init,
    /// Update extension properties in infobase
    Extensions(ExtensionsArgs),
    /// Load sources into infobase
    Build(BuildArgs),
    /// Load release artifacts into infobase
    Load(LoadArgs),
    /// Run YaXUnit tests
    Test(TestArgs),
    /// Dump configuration from infobase to files
    Dump(DumpArgs),
    /// Export configuration artifacts via Designer batch commands
    #[command(name = "make", visible_alias = "artifacts")]
    Artifacts(ArtifactsArgs),
    /// Run syntax checks
    Syntax(SyntaxArgs),
    /// Launch 1C application
    Launch(LaunchArgs),
    /// Run Model Context Protocol transports
    Mcp(McpArgs),
}

#[derive(Args, Debug)]
pub struct BuildArgs {
    /// Clear change cache and rebuild everything
    #[arg(long)]
    pub full_rebuild: bool,
}

#[derive(Args, Debug)]
pub struct LoadArgs {
    /// Path to a built artifact (.cf/.cfe)
    #[arg(long)]
    pub path: String,

    /// Load mode
    #[arg(long, default_value = "load", value_parser = ["load", "merge", "update"])]
    pub mode: String,

    /// Merge settings file used by --mode merge
    #[arg(long)]
    pub settings: Option<String>,

    /// Extension name required for .cfe artifacts
    #[arg(long)]
    pub extension: Option<String>,
}

#[derive(Args, Debug)]
pub struct ExtensionsArgs {
    /// Extension source-set name to update. Repeat to target multiple extensions.
    #[arg(long = "name")]
    pub names: Vec<String>,
}

#[derive(Args, Debug)]
pub struct TestArgs {
    #[arg(long, global = true)]
    pub full: bool,

    #[command(subcommand)]
    pub runner: TestRunner,
}

#[derive(Subcommand, Debug)]
pub enum TestRunner {
    /// Run YaXUnit tests
    Yaxunit(TestYaxunitArgs),
    /// Run Vanessa Automation feature scenarios
    Va,
}

#[derive(Args, Debug)]
pub struct TestYaxunitArgs {
    #[command(subcommand)]
    pub scope: TestScope,
}

#[derive(Subcommand, Debug)]
pub enum TestScope {
    /// Run all tests
    All,
    /// Run tests for a specific module
    Module {
        /// Module name
        name: String,
    },
}

#[derive(Args, Debug)]
pub struct DumpArgs {
    /// Dump mode
    #[arg(long, value_parser = ["full", "incremental", "partial"])]
    pub mode: String,

    /// Source set name
    #[arg(long)]
    pub source_set: Option<String>,

    /// Extension name
    #[arg(long)]
    pub extension: Option<String>,

    /// Objects for partial dump (TYPE:NAME)
    #[arg(long = "object")]
    pub objects: Vec<String>,
}

#[derive(Args, Debug)]
pub struct ArtifactsArgs {
    /// Final output path (.cf/.cfe file or publish directory for external artifacts)
    #[arg(long)]
    pub output: String,

    /// Optional source set name used to disambiguate repository context
    #[arg(long)]
    pub source_set: Option<String>,

    /// Extension name in the infobase for cfe export
    #[arg(long)]
    pub extension: Option<String>,
}

#[derive(Args, Debug)]
pub struct SyntaxArgs {
    #[command(subcommand)]
    pub target: SyntaxTarget,
}

#[derive(Subcommand, Debug)]
pub enum SyntaxTarget {
    /// Check configuration via Designer CheckConfig
    DesignerConfig(DesignerConfigSyntaxArgs),
    /// Check modules via Designer CheckModules
    DesignerModules(DesignerModulesSyntaxArgs),
    /// Check via EDT validate
    Edt {
        /// EDT project names
        #[arg(long = "project")]
        projects: Vec<String>,
    },
}

#[derive(Args, Debug)]
pub struct LaunchArgs {
    /// Launch mode
    #[arg(long, value_parser = ["designer", "thin", "thick"])]
    pub mode: String,
}

#[derive(Args, Debug)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: McpCommand,
}

#[derive(Subcommand, Debug)]
pub enum McpCommand {
    /// Serve an MCP transport
    Serve(McpServeArgs),
}

#[derive(Args, Debug)]
pub struct McpServeArgs {
    #[command(subcommand)]
    pub transport: McpServeTransport,
}

#[derive(Subcommand, Debug)]
pub enum McpServeTransport {
    /// Serve MCP over stdio
    Stdio,
    /// Serve MCP over streamable HTTP
    Http,
}

#[derive(Args, Debug, Clone)]
pub struct DesignerConfigSyntaxArgs {
    #[arg(long)]
    pub config_log_integrity: bool,
    #[arg(long)]
    pub incorrect_references: bool,
    #[arg(long)]
    pub thin_client: bool,
    #[arg(long)]
    pub web_client: bool,
    #[arg(long)]
    pub mobile_client: bool,
    #[arg(long)]
    pub server: bool,
    #[arg(long)]
    pub external_connection: bool,
    #[arg(long)]
    pub external_connection_server: bool,
    #[arg(long)]
    pub mobile_app_client: bool,
    #[arg(long)]
    pub mobile_app_server: bool,
    #[arg(long)]
    pub thick_client_managed_application: bool,
    #[arg(long)]
    pub thick_client_server_managed_application: bool,
    #[arg(long)]
    pub thick_client_ordinary_application: bool,
    #[arg(long)]
    pub thick_client_server_ordinary_application: bool,
    #[arg(long)]
    pub mobile_client_digi_sign: bool,
    #[arg(long)]
    pub distributive_modules: bool,
    #[arg(long)]
    pub unreference_procedures: bool,
    #[arg(long)]
    pub handlers_existence: bool,
    #[arg(long)]
    pub empty_handlers: bool,
    #[arg(long)]
    pub extended_modules_check: bool,
    #[arg(long, requires = "extended_modules_check")]
    pub check_use_synchronous_calls: bool,
    #[arg(long, requires = "extended_modules_check")]
    pub check_use_modality: bool,
    #[arg(long)]
    pub unsupported_functional: bool,
    #[arg(long, conflicts_with = "all_extensions")]
    pub extension: Option<String>,
    #[arg(long)]
    pub all_extensions: bool,
}

#[derive(Args, Debug, Clone)]
pub struct DesignerModulesSyntaxArgs {
    #[arg(long)]
    pub thin_client: bool,
    #[arg(long)]
    pub web_client: bool,
    #[arg(long)]
    pub server: bool,
    #[arg(long)]
    pub external_connection: bool,
    #[arg(long)]
    pub thick_client_ordinary_application: bool,
    #[arg(long)]
    pub mobile_app_client: bool,
    #[arg(long)]
    pub mobile_app_server: bool,
    #[arg(long)]
    pub mobile_client: bool,
    #[arg(long)]
    pub extended_modules_check: bool,
    #[arg(long, conflicts_with = "all_extensions")]
    pub extension: Option<String>,
    #[arg(long)]
    pub all_extensions: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactsArgs, Cli, Command, ExtensionsArgs, LoadArgs, McpCommand, McpServeTransport,
        SyntaxTarget, TestRunner, TestScope,
    };
    use clap::Parser;

    #[test]
    fn syntax_config_extension_conflicts_with_all_extensions() {
        let result = Cli::try_parse_from([
            "v8-test-runner",
            "syntax",
            "designer-config",
            "--extension",
            "Ext",
            "--all-extensions",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn syntax_config_sync_calls_require_extended_modules_check() {
        let result = Cli::try_parse_from([
            "v8-test-runner",
            "syntax",
            "designer-config",
            "--check-use-synchronous-calls",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn parses_init_command() {
        let cli = Cli::try_parse_from(["v8-test-runner", "init"]).expect("parse");
        assert!(matches!(cli.command, Command::Init));
    }

    #[test]
    fn parses_extensions_command_with_names() {
        let cli = Cli::try_parse_from([
            "v8-test-runner",
            "extensions",
            "--name",
            "client_mcp",
            "--name",
            "tests",
        ])
        .expect("parse");

        match cli.command {
            Command::Extensions(ExtensionsArgs { names }) => {
                assert_eq!(names, vec!["client_mcp", "tests"]);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_load_command_with_default_mode() {
        let cli = Cli::try_parse_from(["v8-test-runner", "load", "--path", "dist/main.cf"])
            .expect("parse load");

        match cli.command {
            Command::Load(LoadArgs {
                path,
                mode,
                settings,
                extension,
            }) => {
                assert_eq!(path, "dist/main.cf");
                assert_eq!(mode, "load");
                assert!(settings.is_none());
                assert!(extension.is_none());
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_load_command_with_merge_mode() {
        let cli = Cli::try_parse_from([
            "v8-test-runner",
            "load",
            "--path",
            "dist/ext.cfe",
            "--mode",
            "merge",
            "--settings",
            "merge.xml",
            "--extension",
            "SalesAddon",
        ])
        .expect("parse load merge");

        match cli.command {
            Command::Load(LoadArgs {
                path,
                mode,
                settings,
                extension,
            }) => {
                assert_eq!(path, "dist/ext.cfe");
                assert_eq!(mode, "merge");
                assert_eq!(settings.as_deref(), Some("merge.xml"));
                assert_eq!(extension.as_deref(), Some("SalesAddon"));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_test_yaxunit_module_command() {
        let cli = Cli::try_parse_from(["v8-test-runner", "test", "yaxunit", "module", "Foo"])
            .expect("parse test yaxunit");

        match cli.command {
            Command::Test(args) => {
                assert!(!args.full);
                match args.runner {
                    TestRunner::Yaxunit(yaxunit) => {
                        assert!(
                            matches!(yaxunit.scope, TestScope::Module { name } if name == "Foo")
                        );
                    }
                    _ => panic!("unexpected test runner"),
                }
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_test_va_command() {
        let cli = Cli::try_parse_from(["v8-test-runner", "test", "va"]).expect("parse test va");

        match cli.command {
            Command::Test(args) => {
                assert!(matches!(args.runner, TestRunner::Va));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn syntax_modules_all_extensions_conflicts_with_extension() {
        let result = Cli::try_parse_from([
            "v8-test-runner",
            "syntax",
            "designer-modules",
            "--server",
            "--extension",
            "Ext",
            "--all-extensions",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn syntax_config_accepts_zero_mode_flags() {
        let cli = Cli::try_parse_from(["v8-test-runner", "syntax", "designer-config"])
            .expect("parse syntax config");

        match cli.command {
            Command::Syntax(args) => match args.target {
                SyntaxTarget::DesignerConfig(config) => {
                    assert!(!config.server);
                    assert!(!config.all_extensions);
                }
                _ => panic!("unexpected syntax target"),
            },
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_mcp_stdio_command() {
        let cli = Cli::try_parse_from(["v8-test-runner", "mcp", "serve", "stdio"])
            .expect("parse mcp stdio");

        match cli.command {
            Command::Mcp(args) => match args.command {
                McpCommand::Serve(serve) => {
                    assert!(matches!(serve.transport, McpServeTransport::Stdio));
                }
            },
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_mcp_http_command() {
        let cli = Cli::try_parse_from(["v8-test-runner", "mcp", "serve", "http"])
            .expect("parse mcp http");

        match cli.command {
            Command::Mcp(args) => match args.command {
                McpCommand::Serve(serve) => {
                    assert!(matches!(serve.transport, McpServeTransport::Http));
                }
            },
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_make_cf_command() {
        let cli = Cli::try_parse_from(["v8-test-runner", "make", "--output", "dist/main.cf"])
            .expect("parse make");

        match cli.command {
            Command::Artifacts(ArtifactsArgs {
                output,
                source_set,
                extension,
            }) => {
                assert_eq!(output, "dist/main.cf");
                assert!(source_set.is_none());
                assert!(extension.is_none());
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_make_cfe_command_with_extension_and_source_set() {
        let cli = Cli::try_parse_from([
            "v8-test-runner",
            "make",
            "--output",
            "dist/ext.cfe",
            "--source-set",
            "ext-sales",
            "--extension",
            "SalesAddon",
        ])
        .expect("parse make");

        match cli.command {
            Command::Artifacts(ArtifactsArgs {
                output,
                source_set,
                extension,
            }) => {
                assert_eq!(output, "dist/ext.cfe");
                assert_eq!(source_set.as_deref(), Some("ext-sales"));
                assert_eq!(extension.as_deref(), Some("SalesAddon"));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_artifacts_alias_command() {
        let cli = Cli::try_parse_from(["v8-test-runner", "artifacts", "--output", "dist/main.cf"])
            .expect("parse artifacts alias");

        assert!(matches!(cli.command, Command::Artifacts(_)));
    }
}
