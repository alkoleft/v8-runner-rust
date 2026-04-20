use clap::Parser;
use tracing::{debug, error};

use crate::cli::args::{
    Cli, Command, ConfigCommand, ConfigInitArgs, McpCommand, McpServeTransport,
};
use crate::cli::execute;
use crate::config::loader::load_config;
use crate::output::json::Envelope;
use crate::output::presenter::Presenter;
use crate::use_cases::config_init::{ConfigBuilderRequest, ConfigFormatRequest, ConfigInitRequest};

pub fn run() -> i32 {
    let cli = Cli::parse();

    if let Command::Mcp(args) = &cli.command {
        return run_mcp_command(&cli, args);
    }

    let color_mode = if cli.no_color {
        crate::output::presenter::ColorMode::Disabled
    } else {
        crate::output::presenter::ColorMode::Enabled
    };
    let presenter = Presenter::new(cli.output.clone(), color_mode);

    if let Command::Config(args) = &cli.command {
        return run_config_command(&cli, args, &presenter);
    }

    let config = match load_config(cli.config.as_deref(), cli.workdir.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            presenter.print_error(&format!("{e}"));
            return crate::output::exit_codes::VALIDATION_ERROR;
        }
    };

    let level = cli.log_level.as_deref().unwrap_or("info");
    let action_log_path = match crate::support::logging::init_action_logging(
        level,
        &cli.output,
        !cli.no_color,
        &config.work_path,
    ) {
        Ok(path) => path,
        Err(e) => {
            presenter.print_error(&format!("{e}"));
            return crate::output::exit_codes::RUNTIME_ERROR;
        }
    };

    debug!(
        command = command_name(&cli.command),
        output = cli.output.as_str(),
        work_path = %config.work_path.display(),
        "starting command"
    );
    if let Some(path) = &action_log_path {
        debug!(path = %path.display(), "action log file enabled");
    }

    let result = match &cli.command {
        Command::Init
        | Command::Config(_)
        | Command::Extensions(_)
        | Command::Build(_)
        | Command::Load(_)
        | Command::Test(_)
        | Command::Dump(_)
        | Command::Artifacts(_)
        | Command::Syntax(_)
        | Command::Launch(_) => execute::execute_command(
            &config,
            &cli.command,
            &presenter,
            cli.clean_before_execution,
        ),
        Command::Mcp(_) => unreachable!("mcp commands are handled before CLI presenter setup"),
    };

    match result {
        Ok(()) => {
            debug!(
                command = command_name(&cli.command),
                "command finished successfully"
            );
            0
        }
        Err(e) => {
            // Text command adapters have already rendered the error; text action logs
            // go to stdout, so logging here would duplicate user-facing output.
            if presenter.is_json() {
                error!("{e}");
            }
            e.exit_code()
        }
    }
}

fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Config(_) => "config",
        _ => execute::command_name(command).as_str(),
    }
}

fn run_config_command(
    cli: &Cli,
    args: &crate::cli::args::ConfigArgs,
    presenter: &Presenter,
) -> i32 {
    match &args.command {
        ConfigCommand::Init(init_args) => run_config_init(cli, init_args, presenter),
    }
}

fn run_config_init(cli: &Cli, args: &ConfigInitArgs, presenter: &Presenter) -> i32 {
    let project_dir = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            presenter.print_error(&format!("failed to resolve current directory: {error}"));
            return crate::output::exit_codes::RUNTIME_ERROR;
        }
    };
    let output_path = args
        .file
        .as_deref()
        .or(cli.config.as_deref())
        .unwrap_or("v8project.yaml");

    let request = ConfigInitRequest {
        project_dir,
        output_path: output_path.into(),
        force: args.force,
        connection: args.connection.clone(),
        format: map_config_format(&args.format),
        builder: map_config_builder(&args.builder),
    };

    match crate::use_cases::config_init::execute(&request) {
        Ok(result) => {
            if presenter.is_json() {
                presenter.print_envelope(&Envelope::ok("config init", result.duration_ms, result));
            } else {
                presenter.print_ok(&format!("Config written: {}", result.path));
                for source_set in &result.source_sets {
                    presenter.print_success_item(&format!(
                        "{}: {} ({})",
                        source_set.name, source_set.path, source_set.source_type
                    ));
                }
            }
            0
        }
        Err(error) => {
            presenter.print_error(&error.to_string());
            match error {
                crate::support::error::AppError::Validation(_) => {
                    crate::output::exit_codes::VALIDATION_ERROR
                }
                _ => crate::output::exit_codes::RUNTIME_ERROR,
            }
        }
    }
}

fn map_config_format(value: &str) -> ConfigFormatRequest {
    match value {
        "designer" | "DESIGNER" => ConfigFormatRequest::Designer,
        "edt" | "EDT" => ConfigFormatRequest::Edt,
        _ => ConfigFormatRequest::Auto,
    }
}

fn map_config_builder(value: &str) -> ConfigBuilderRequest {
    match value {
        "ibcmd" | "IBCMD" => ConfigBuilderRequest::Ibcmd,
        _ => ConfigBuilderRequest::Designer,
    }
}

fn run_mcp_command(cli: &Cli, args: &crate::cli::args::McpArgs) -> i32 {
    match &args.command {
        McpCommand::Serve(serve) => match serve.transport {
            McpServeTransport::Stdio => run_mcp_stdio(cli),
            McpServeTransport::Http => run_mcp_http(cli),
        },
    }
}

fn run_mcp_stdio(cli: &Cli) -> i32 {
    install_mcp_panic_hook();

    let config = match prepare_mcp_runtime(cli, "stdio") {
        Ok(config) => config,
        Err(exit_code) => return exit_code,
    };

    match crate::mcp::server::serve_stdio(config) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            crate::output::exit_codes::RUNTIME_ERROR
        }
    }
}

fn run_mcp_http(cli: &Cli) -> i32 {
    install_mcp_panic_hook();

    let config = match prepare_mcp_runtime(cli, "http") {
        Ok(config) => config,
        Err(exit_code) => return exit_code,
    };

    match crate::mcp::server::serve_http(config) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            crate::output::exit_codes::RUNTIME_ERROR
        }
    }
}

fn prepare_mcp_runtime(
    cli: &Cli,
    transport: &'static str,
) -> Result<crate::config::model::AppConfig, i32> {
    let config = match load_config(cli.config.as_deref(), cli.workdir.as_deref()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return Err(crate::output::exit_codes::VALIDATION_ERROR);
        }
    };

    if cli.clean_before_execution {
        eprintln!("--clean-before-execution is not supported for MCP transports");
        return Err(crate::output::exit_codes::VALIDATION_ERROR);
    }

    let level = cli.log_level.as_deref().unwrap_or("info");
    if let Err(error) =
        crate::support::logging::init_action_logging(level, "json", false, &config.work_path)
    {
        eprintln!("{error}");
        return Err(crate::output::exit_codes::RUNTIME_ERROR);
    }

    debug!(
        transport,
        work_path = %config.work_path.display(),
        "starting mcp server"
    );

    Ok(config)
}

fn install_mcp_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        eprintln!("{panic_info}");
    }));
}
