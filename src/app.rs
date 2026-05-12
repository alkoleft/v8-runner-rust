use clap::Parser;
use tracing::{debug, error};

use crate::cli::args::{
    Cli, Command, ConfigCommand, ConfigInitArgs, McpCommand, McpServeTransport, ToolsCommand,
};
use crate::cli::execute;
use crate::cli::output::print_command_error;
use crate::command_envelope::Envelope;
use crate::config::loader::{
    load_config, load_config_for_tools_download, resolve_primary_config_path,
};
use crate::output::presenter::Presenter;
use crate::output::text::{TimelineItem, TimelineStatus};
use crate::support::error::AppError;
use crate::use_cases::config_init::{ConfigBuilderRequest, ConfigFormatRequest, ConfigInitRequest};
use crate::use_cases::result::{UseCaseError, UseCaseErrorKind};

const CONFIG_INIT_COMMAND: &str = "config init";

pub fn run() -> i32 {
    let cli = Cli::parse();
    let output_format = cli_output_format(cli.json_message);

    if let Command::Mcp(args) = &cli.command {
        return run_mcp_command(&cli, args);
    }

    let color_mode = if cli.no_color {
        crate::output::presenter::ColorMode::Disabled
    } else {
        crate::output::presenter::ColorMode::Enabled
    };
    let presenter = Presenter::new(output_format.to_owned(), color_mode);

    if let Command::Config(args) = &cli.command {
        return run_config_command(args, &presenter);
    }

    let config = match load_cli_config(&cli) {
        Ok(c) => c,
        Err(e) => {
            let message = e.to_string();
            let error = UseCaseError::from(AppError::from(e));
            print_command_error(&presenter, command_name(&cli.command), &error, &message);
            return error.exit_code();
        }
    };
    let primary_config_path = match resolve_primary_config_path(cli.config.as_deref()) {
        Ok(path) => path,
        Err(e) => {
            let message = e.to_string();
            let error = UseCaseError::from(AppError::from(e));
            print_command_error(&presenter, command_name(&cli.command), &error, &message);
            return error.exit_code();
        }
    };

    let level = cli.log_level.as_deref().unwrap_or("info");
    let action_log_path = match crate::support::logging::init_action_logging(
        level,
        output_format,
        !cli.no_color,
        &config.work_path,
    ) {
        Ok(path) => path,
        Err(e) => {
            let message = e.to_string();
            let error = UseCaseError::new(UseCaseErrorKind::Runtime, message.clone());
            print_command_error(&presenter, command_name(&cli.command), &error, &message);
            return error.exit_code();
        }
    };

    debug!(
        command = command_name(&cli.command),
        output = output_format,
        work_path = %config.work_path.display(),
        "starting command"
    );
    if let Some(path) = &action_log_path {
        debug!(path = %path.display(), "action log file enabled");
    }

    let result = match &cli.command {
        Command::Init
        | Command::Config(_)
        | Command::Tools(_)
        | Command::Extensions(_)
        | Command::Build(_)
        | Command::Load(_)
        | Command::Test(_)
        | Command::Dump(_)
        | Command::Convert(_)
        | Command::Artifacts(_)
        | Command::Syntax(_)
        | Command::Launch(_) => execute::execute_command(
            &config,
            &cli.command,
            Some(primary_config_path),
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

fn load_cli_config(
    cli: &Cli,
) -> Result<crate::config::model::AppConfig, crate::config::loader::ConfigLoadError> {
    if matches!(
        &cli.command,
        Command::Tools(crate::cli::args::ToolsArgs {
            command: ToolsCommand::Download(_)
        })
    ) {
        load_config_for_tools_download(cli.config.as_deref(), cli.workdir.as_deref())
    } else {
        load_config(cli.config.as_deref(), cli.workdir.as_deref())
    }
}

fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Config(_) => "config",
        _ => execute::command_name(command).as_str(),
    }
}

fn run_config_command(args: &crate::cli::args::ConfigArgs, presenter: &Presenter) -> i32 {
    match &args.command {
        ConfigCommand::Init(init_args) => run_config_init(init_args, presenter),
    }
}

fn run_config_init(args: &ConfigInitArgs, presenter: &Presenter) -> i32 {
    if config_flag_was_explicitly_set() {
        let message =
            "global --config flag is not supported for `config init`; use `config init --output <FILE>` to choose where the generated config is written";
        let error = UseCaseError::new(UseCaseErrorKind::Validation, message);
        print_command_error(presenter, CONFIG_INIT_COMMAND, &error, message);
        return error.exit_code();
    }

    let project_dir = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            let message = format!("failed to resolve current directory: {error}");
            let error = UseCaseError::new(UseCaseErrorKind::Runtime, message.clone());
            print_command_error(presenter, CONFIG_INIT_COMMAND, &error, &message);
            return error.exit_code();
        }
    };
    let output_path = args.output.as_deref().unwrap_or("v8project.yaml");

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
                presenter.print_envelope(&Envelope {
                    ok: true,
                    command: CONFIG_INIT_COMMAND.to_owned(),
                    duration_ms: result.duration_ms,
                    warnings: result.warnings.clone(),
                    steps: Vec::new(),
                    error: None,
                    data: result,
                });
            } else {
                render_config_init_text(&result, presenter);
            }
            0
        }
        Err(error) => {
            let message = error.to_string();
            let error = UseCaseError::from(error);
            print_command_error(presenter, CONFIG_INIT_COMMAND, &error, &message);
            error.exit_code()
        }
    }
}

fn render_config_init_text(
    result: &crate::domain::config_init::ConfigInitResult,
    presenter: &Presenter,
) {
    let mut details = vec![
        format!("path: {}", result.path),
        format!("local path: {}", result.local_path),
        format!("gitignore: {}", result.gitignore_path),
        format!("format: {}", result.format),
        format!("builder: {}", result.builder),
    ];
    if result.overwritten {
        details.push("overwritten: yes".to_owned());
    }
    if let Some(platform_version) = result.platform_version.as_deref() {
        details.push(format!("platform version: {platform_version}"));
    }
    for source_set in &result.source_sets {
        details.push(format!(
            "source-set {}: {} ({})",
            source_set.name, source_set.path, source_set.source_type
        ));
    }
    for warning in &result.warnings {
        details.push(format!("[warning] {warning}"));
    }

    let completion = if result.warnings.is_empty() {
        "Config written successfully"
    } else {
        "Config written with warnings"
    };
    let timeline = vec![
        TimelineItem::new(TimelineStatus::Succeeded, "config:").with_detail(details.join("\n")),
        TimelineItem::new(TimelineStatus::Succeeded, completion),
    ];
    presenter.print_timeline(&timeline);
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

fn cli_output_format(json_message: bool) -> &'static str {
    if json_message {
        "json"
    } else {
        "text"
    }
}

fn config_flag_was_explicitly_set() -> bool {
    std::env::args_os().skip(1).any(|arg| {
        let value = arg.to_string_lossy();
        value == "--config" || value.starts_with("--config=")
    })
}
