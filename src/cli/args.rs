use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "v8-runner",
    about = "Run 1C:Enterprise build, test, dump, convert, and launch workflows"
)]
pub struct Cli {
    /// Path to an existing YAML config file. Defaults to ./v8project.yaml
    #[arg(
        long,
        global = true,
        env = "V8TR_CONFIG",
        help_heading = "Global options"
    )]
    pub config: Option<String>,

    /// Print structured JSON envelopes instead of text output
    #[arg(long, global = true, help_heading = "Global options")]
    pub json_message: bool,

    /// Log level
    #[arg(long, global = true, default_value = "info",
          value_parser = ["error", "warn", "info", "debug", "trace"],
          help_heading = "Global options")]
    pub log_level: Option<String>,

    /// Clear log files before execution
    #[arg(long, global = true, help_heading = "Global options")]
    pub clean_before_execution: bool,

    /// Disable ANSI colors
    #[arg(long, global = true, help_heading = "Global options")]
    pub no_color: bool,

    /// Override working directory
    #[arg(long, global = true, help_heading = "Global options")]
    pub workdir: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Generate project configuration and autodetect source-sets
    Config(ConfigArgs),
    /// Download YaXUnit, Vanessa Automation, and client MCP tool assets
    Tools(ToolsArgs),
    /// Initialize the infobase and EDT workspace
    Init,
    /// Update configured extension properties inside the infobase
    Extensions(ExtensionsArgs),
    /// Build configured source-sets into the infobase
    Build(BuildArgs),
    /// Apply built release artifacts to the infobase
    Load(LoadArgs),
    /// Build first, then run YaXUnit or Vanessa Automation tests
    Test(TestArgs),
    /// Dump infobase state back to project files
    Dump(DumpArgs),
    /// Convert configured source-sets between EDT and Designer file formats
    Convert(ConvertArgs),
    /// Export release artifacts via Designer batch commands
    #[command(name = "make", visible_alias = "artifacts")]
    Artifacts(ArtifactsArgs),
    /// Run Designer or EDT syntax validation
    Syntax(SyntaxArgs),
    /// Launch 1C application
    Launch(LaunchArgs),
    /// Serve Model Context Protocol transports
    Mcp(McpArgs),
}

#[derive(Args, Debug)]
pub struct ToolsArgs {
    #[command(subcommand)]
    pub command: ToolsCommand,
}

#[derive(Subcommand, Debug)]
pub enum ToolsCommand {
    /// Download a supported test or MCP helper tool from its latest GitHub release
    Download(ToolsDownloadArgs),
}

#[derive(Args, Debug)]
#[command(next_help_heading = "Command options")]
pub struct ToolsDownloadArgs {
    #[command(subcommand)]
    pub command: ToolsDownloadCommand,
}

#[derive(Subcommand, Debug)]
pub enum ToolsDownloadCommand {
    /// Download YAxUnit extension assets or sources
    Yaxunit(ToolsDownloadExtensionArgs),
    /// Download Vanessa Automation Single external processor
    #[command(visible_alias = "vanessa-automation-single")]
    Vanessa(ToolsDownloadToolArgs),
    /// Download onec-client-mcp-devkit extension assets or sources
    #[command(name = "client-mcp", visible_alias = "client_mcp")]
    ClientMcp(ToolsDownloadExtensionArgs),
}

#[derive(Args, Debug)]
#[command(next_help_heading = "Command options")]
pub struct ToolsDownloadExtensionArgs {
    /// Download extension sources instead of the release artifact
    #[arg(long)]
    pub sources: bool,

    /// Re-download managed targets created by tools download
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
#[command(next_help_heading = "Command options")]
pub struct ToolsDownloadToolArgs {
    /// Re-download managed targets created by tools download
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Create a new config file and add detected sources
    Init(ConfigInitArgs),
}

#[derive(Args, Debug)]
#[command(next_help_heading = "Command options")]
pub struct ConfigInitArgs {
    /// Overwrite an existing config file
    #[arg(long)]
    pub force: bool,

    /// Path to the generated YAML config file. Defaults to ./v8project.yaml
    #[arg(long)]
    pub output: Option<String>,

    /// Infobase connection string written to config
    #[arg(long)]
    pub connection: Option<String>,

    /// Source format to write
    #[arg(long, default_value = "auto", value_parser = ["auto", "designer", "edt"])]
    pub format: String,

    /// Builder backend to write
    #[arg(long, default_value = "DESIGNER", value_parser = ["DESIGNER", "IBCMD", "designer", "ibcmd"])]
    pub builder: String,
}

#[derive(Args, Debug)]
#[command(next_help_heading = "Command options")]
pub struct BuildArgs {
    /// Clear change cache and rebuild everything
    #[arg(long)]
    pub full_rebuild: bool,

    /// Limit build to one source-set from v8project.yaml
    #[arg(long)]
    pub source_set: Option<String>,

    /// Apply changes via `/UpdateDBCfg -Dynamic+` (no exclusive lock).
    ///
    /// Overrides `build.dynamicUpdate` from v8project.yaml for this run. The platform itself
    /// refuses dynamic mode when restructuring is required; the runner surfaces that error
    /// instead of falling back to a static update.
    #[arg(long)]
    pub dynamic: bool,
}

#[derive(Args, Debug)]
#[command(next_help_heading = "Command options")]
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
#[command(next_help_heading = "Command options")]
pub struct ExtensionsArgs {
    /// Extension source-set name to update. Repeat to target multiple extensions.
    #[arg(long = "name")]
    pub names: Vec<String>,
}

#[derive(Args, Debug)]
#[command(next_help_heading = "Command options")]
pub struct TestArgs {
    #[arg(long, global = true)]
    pub full: bool,

    /// Client mode used for enterprise launch during test execution
    #[arg(long = "client-mode", value_parser = ["designer", "thin", "thick", "ordinary"])]
    pub client_mode: Option<String>,

    #[command(flatten)]
    pub launch: LaunchOptionsArgs,

    #[command(subcommand)]
    pub runner: TestRunner,
}

#[derive(Subcommand, Debug)]
pub enum TestRunner {
    /// Run YaXUnit tests
    Yaxunit(TestYaxunitArgs),
    /// Run Vanessa Automation feature scenarios
    Va(TestVaArgs),
}

#[derive(Args, Debug)]
#[command(next_help_heading = "Command options")]
pub struct TestYaxunitArgs {
    #[command(subcommand)]
    pub scope: TestScope,
}

#[derive(Args, Debug, Default)]
#[command(next_help_heading = "Vanessa Automation options")]
pub struct TestVaArgs {
    /// Feature name from the selected Vanessa profile to run. Repeat to run multiple features.
    #[arg(long = "feature")]
    pub features_to_run: Vec<String>,

    /// Tag expression to include. Repeat to pass multiple include tags.
    #[arg(long = "filter-tag")]
    pub filter_tags: Vec<String>,

    /// Tag expression to exclude. Repeat to pass multiple exclude tags.
    #[arg(long = "ignore-tag")]
    pub ignore_tags: Vec<String>,

    /// Scenario name filter. Repeat to pass multiple scenario filters.
    #[arg(long = "scenario-filter")]
    pub scenario_filter: Vec<String>,
}

impl TestVaArgs {
    pub fn has_profile_overrides(&self) -> bool {
        !self.features_to_run.is_empty()
            || !self.filter_tags.is_empty()
            || !self.ignore_tags.is_empty()
            || !self.scenario_filter.is_empty()
    }
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
#[command(next_help_heading = "Command options")]
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
#[command(next_help_heading = "Command options")]
pub struct ConvertArgs {
    /// Limit conversion to one source-set from v8project.yaml
    #[arg(long)]
    pub source_set: Option<String>,

    /// Target root for converted source-set layout. Defaults to workPath/convert/out
    #[arg(long)]
    pub output: Option<String>,
}

#[derive(Args, Debug)]
#[command(next_help_heading = "Command options")]
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
#[command(next_help_heading = "Command options")]
pub struct LaunchArgs {
    /// Launch mode
    #[arg(value_name = "MODE", value_parser = ["designer", "thin", "thick", "ordinary", "mcp"])]
    pub target: String,

    /// Optional client-side MCP scenario to start with the MCP server
    #[arg(value_name = "MCP_SCENARIO", value_parser = ["va"])]
    pub mcp_scenario: Option<String>,

    /// 1C client mode for `launch mcp`
    #[arg(long = "mode", value_parser = ["thin", "thick", "ordinary"])]
    pub mcp_mode: Option<String>,

    #[command(flatten)]
    pub launch: LaunchOptionsArgs,

    /// JSON config path for onec-client-mcp-devkit `/C"runMcp=<FILE>"`
    #[arg(long = "mcp-config")]
    pub mcp_config: Option<String>,

    /// Port override for onec-client-mcp-devkit `/C"...;mcpPort=<PORT>"`
    #[arg(long = "mcp-port")]
    pub mcp_port: Option<u16>,
}

#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
#[command(next_help_heading = "Command options")]
pub struct LaunchOptionsArgs {
    /// Value for `/C`
    #[arg(long = "c")]
    pub c: Option<String>,
    /// Value for `/Execute`
    #[arg(long)]
    pub execute: Option<String>,
    /// Enables `/UsePrivilegedMode`
    #[arg(long = "use-privileged-mode")]
    pub use_privileged_mode: bool,
    /// User-provided `/Out` path allowed only for direct launch
    #[arg(long)]
    pub output: Option<String>,
    /// Additional raw launch arguments appended after typed launch keys
    #[arg(long = "raw-key")]
    pub raw_keys: Vec<String>,
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
#[command(next_help_heading = "Command options")]
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
#[command(next_help_heading = "Command options")]
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
        ArtifactsArgs, Cli, Command, ConvertArgs, ExtensionsArgs, LaunchArgs, LaunchOptionsArgs,
        LoadArgs, McpCommand, McpServeTransport, SyntaxTarget, TestRunner, TestScope,
    };
    use clap::Parser;

    #[test]
    fn syntax_config_extension_conflicts_with_all_extensions() {
        let result = Cli::try_parse_from([
            "v8-runner",
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
            "v8-runner",
            "syntax",
            "designer-config",
            "--check-use-synchronous-calls",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn parses_init_command() {
        let cli = Cli::try_parse_from(["v8-runner", "init"]).expect("parse");
        assert!(matches!(cli.command, Command::Init));
    }

    #[test]
    fn parses_extensions_command_with_names() {
        let cli = Cli::try_parse_from([
            "v8-runner",
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
        let cli = Cli::try_parse_from(["v8-runner", "load", "--path", "dist/main.cf"])
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
            "v8-runner",
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
        let cli = Cli::try_parse_from(["v8-runner", "test", "yaxunit", "module", "Foo"])
            .expect("parse test yaxunit");

        match cli.command {
            Command::Test(args) => {
                assert!(!args.full);
                assert_eq!(args.launch.raw_keys, Vec::<String>::new());
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
        let cli = Cli::try_parse_from(["v8-runner", "test", "va"]).expect("parse test va");

        match cli.command {
            Command::Test(args) => {
                assert!(matches!(args.runner, TestRunner::Va(_)));
                assert_eq!(args.launch, LaunchOptionsArgs::default());
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_test_va_filter_options() {
        let cli = Cli::try_parse_from([
            "v8-runner",
            "test",
            "va",
            "--feature",
            "login",
            "--filter-tag",
            "@smoke",
            "--ignore-tag",
            "@draft",
            "--scenario-filter",
            "Проверка логина",
        ])
        .expect("parse test va filters");

        match cli.command {
            Command::Test(args) => match args.runner {
                TestRunner::Va(va) => {
                    assert_eq!(va.features_to_run, ["login"]);
                    assert_eq!(va.filter_tags, ["@smoke"]);
                    assert_eq!(va.ignore_tags, ["@draft"]);
                    assert_eq!(va.scenario_filter, ["Проверка логина"]);
                }
                _ => panic!("unexpected test runner"),
            },
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_test_command_with_launch_options() {
        let cli = Cli::try_parse_from([
            "v8-runner",
            "test",
            "--c",
            "RunUnitTests=config.json",
            "--use-privileged-mode",
            "--raw-key",
            "/WA-",
            "yaxunit",
            "all",
        ])
        .expect("parse test");

        match cli.command {
            Command::Test(args) => {
                assert_eq!(
                    args.launch,
                    LaunchOptionsArgs {
                        c: Some("RunUnitTests=config.json".to_owned()),
                        execute: None,
                        use_privileged_mode: true,
                        output: None,
                        raw_keys: vec!["/WA-".to_owned()],
                    }
                );
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_launch_command_with_typed_and_raw_keys() {
        let cli = Cli::try_parse_from([
            "v8-runner",
            "launch",
            "ordinary",
            "--c",
            "DoWork",
            "--execute",
            "tool.epf",
            "--use-privileged-mode",
            "--output",
            "launch.log",
            "--raw-key",
            "/WA-",
            "--raw-key",
            "/DisplayAllFunctions",
        ])
        .expect("parse launch");

        match cli.command {
            Command::Launch(LaunchArgs {
                target,
                launch,
                mcp_scenario,
                mcp_mode,
                mcp_config,
                mcp_port,
            }) => {
                assert_eq!(target, "ordinary");
                assert_eq!(launch.c.as_deref(), Some("DoWork"));
                assert_eq!(launch.execute.as_deref(), Some("tool.epf"));
                assert!(launch.use_privileged_mode);
                assert_eq!(launch.output.as_deref(), Some("launch.log"));
                assert_eq!(launch.raw_keys, vec!["/WA-", "/DisplayAllFunctions"]);
                assert_eq!(mcp_scenario, None);
                assert_eq!(mcp_mode, None);
                assert_eq!(mcp_config, None);
                assert_eq!(mcp_port, None);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_global_json_message_flag() {
        let cli = Cli::try_parse_from(["v8-runner", "--json-message", "build"]).expect("parse");
        assert!(cli.json_message);
    }

    #[test]
    fn parses_config_init_output_override() {
        let cli = Cli::try_parse_from(["v8-runner", "config", "init", "--output", "custom.yaml"])
            .expect("parse config init");

        match cli.command {
            Command::Config(config) => match config.command {
                super::ConfigCommand::Init(args) => {
                    assert_eq!(args.output.as_deref(), Some("custom.yaml"));
                }
            },
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_launch_command_with_positional_mode() {
        let cli = Cli::try_parse_from(["v8-runner", "launch", "designer"]).expect("parse launch");

        match cli.command {
            Command::Launch(LaunchArgs {
                target,
                launch,
                mcp_scenario,
                mcp_mode,
                mcp_config,
                mcp_port,
            }) => {
                assert_eq!(target, "designer");
                assert_eq!(launch, LaunchOptionsArgs::default());
                assert_eq!(mcp_scenario, None);
                assert_eq!(mcp_mode, None);
                assert_eq!(mcp_config, None);
                assert_eq!(mcp_port, None);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_launch_mcp_command_with_mcp_options() {
        let cli = Cli::try_parse_from([
            "v8-runner",
            "launch",
            "mcp",
            "va",
            "--mode",
            "ordinary",
            "--mcp-config",
            "mcp-conf.json",
            "--mcp-port",
            "9876",
        ])
        .expect("parse launch");

        match cli.command {
            Command::Launch(LaunchArgs {
                target,
                launch,
                mcp_scenario,
                mcp_mode,
                mcp_config,
                mcp_port,
            }) => {
                assert_eq!(target, "mcp");
                assert_eq!(launch, LaunchOptionsArgs::default());
                assert_eq!(mcp_scenario.as_deref(), Some("va"));
                assert_eq!(mcp_mode.as_deref(), Some("ordinary"));
                assert_eq!(mcp_config.as_deref(), Some("mcp-conf.json"));
                assert_eq!(mcp_port, Some(9876));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn syntax_modules_all_extensions_conflicts_with_extension() {
        let result = Cli::try_parse_from([
            "v8-runner",
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
        let cli = Cli::try_parse_from(["v8-runner", "syntax", "designer-config"])
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
        let cli =
            Cli::try_parse_from(["v8-runner", "mcp", "serve", "stdio"]).expect("parse mcp stdio");

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
        let cli =
            Cli::try_parse_from(["v8-runner", "mcp", "serve", "http"]).expect("parse mcp http");

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
        let cli = Cli::try_parse_from(["v8-runner", "make", "--output", "dist/main.cf"])
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
    fn parses_convert_without_source_set() {
        let cli = Cli::try_parse_from(["v8-runner", "convert"]).expect("parse convert");

        match cli.command {
            Command::Convert(ConvertArgs { source_set, output }) => {
                assert!(source_set.is_none());
                assert!(output.is_none());
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_convert_with_source_set() {
        let cli = Cli::try_parse_from(["v8-runner", "convert", "--source-set", "ext-sales"])
            .expect("parse convert");

        match cli.command {
            Command::Convert(ConvertArgs { source_set, output }) => {
                assert_eq!(source_set.as_deref(), Some("ext-sales"));
                assert!(output.is_none());
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_convert_with_output_root() {
        let cli = Cli::try_parse_from(["v8-runner", "convert", "--output", "tests/fixtures/edt"])
            .expect("parse convert");

        match cli.command {
            Command::Convert(ConvertArgs { source_set, output }) => {
                assert!(source_set.is_none());
                assert_eq!(output.as_deref(), Some("tests/fixtures/edt"));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_make_cfe_command_with_extension_and_source_set() {
        let cli = Cli::try_parse_from([
            "v8-runner",
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
        let cli = Cli::try_parse_from(["v8-runner", "artifacts", "--output", "dist/main.cf"])
            .expect("parse artifacts alias");

        assert!(matches!(cli.command, Command::Artifacts(_)));
    }
}
