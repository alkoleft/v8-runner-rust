use clap::Parser;
use tracing::{error, info};

use crate::cli::args::{Cli, Command, McpCommand, McpServeTransport};
use crate::cli::execute;
use crate::config::loader::load_config;
use crate::output::presenter::Presenter;

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

    let config = match load_config(cli.config.as_deref(), cli.workdir.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            presenter.print_error(&format!("{e}"));
            return crate::output::exit_codes::VALIDATION_ERROR;
        }
    };

    let level = cli.log_level.as_deref().unwrap_or("info");
    let action_log_path =
        match crate::support::logging::init_action_logging(level, &cli.output, &config.work_path) {
            Ok(path) => path,
            Err(e) => {
                presenter.print_error(&format!("{e}"));
                return crate::output::exit_codes::RUNTIME_ERROR;
            }
        };

    info!(
        command = command_name(&cli.command),
        output = cli.output.as_str(),
        work_path = %config.work_path.display(),
        "starting command"
    );
    if let Some(path) = &action_log_path {
        info!(path = %path.display(), "action log file enabled");
    }

    if cli.clean_before_execution {
        info!("cleaning platform logs directory before execution");
        match crate::support::temp::platform_logs_dir(&config.work_path)
            .and_then(|dir| crate::support::fs::clean_dir(&dir))
        {
            Ok(()) => info!("platform logs directory cleaned"),
            Err(e) => {
                presenter.print_error(&format!("failed to clean platform logs: {e}"));
                return crate::output::exit_codes::RUNTIME_ERROR;
            }
        }
    }

    let result = match &cli.command {
        Command::Build(_)
        | Command::Test(_)
        | Command::Dump(_)
        | Command::Syntax(_)
        | Command::Launch(_) => execute::execute_command(&config, &cli.command, &presenter),
        Command::Mcp(_) => unreachable!("mcp commands are handled before CLI presenter setup"),
    };

    match result {
        Ok(()) => {
            info!(
                command = command_name(&cli.command),
                "command finished successfully"
            );
            0
        }
        Err(e) => {
            error!("{e}");
            e.exit_code()
        }
    }
}

fn command_name(command: &Command) -> &'static str {
    execute::command_name(command).as_str()
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

    let level = cli.log_level.as_deref().unwrap_or("info");
    if let Err(error) =
        crate::support::logging::init_action_logging(level, "json", &config.work_path)
    {
        eprintln!("{error}");
        return Err(crate::output::exit_codes::RUNTIME_ERROR);
    }

    info!(
        transport,
        work_path = %config.work_path.display(),
        "starting mcp server"
    );

    if cli.clean_before_execution {
        match crate::support::temp::platform_logs_dir(&config.work_path)
            .and_then(|dir| crate::support::fs::clean_dir(&dir))
        {
            Ok(()) => info!(
                transport,
                "platform logs directory cleaned for mcp transport"
            ),
            Err(error) => {
                eprintln!("failed to clean platform logs: {error}");
                return Err(crate::output::exit_codes::RUNTIME_ERROR);
            }
        }
    }

    Ok(config)
}

fn install_mcp_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        eprintln!("{panic_info}");
    }));
}
