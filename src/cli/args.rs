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
    /// Load sources into infobase
    Build(BuildArgs),
    /// Run YaXUnit tests
    Test(TestArgs),
    /// Dump configuration from infobase to files
    Dump(DumpArgs),
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
pub struct TestArgs {
    #[arg(long)]
    pub full: bool,

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
    use super::{Cli, Command, McpCommand, McpServeTransport, SyntaxTarget};
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
}
