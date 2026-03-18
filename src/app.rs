use clap::Parser;
use tracing::error;

use crate::cli::args::{Cli, Command};
use crate::config::loader::load_config;
use crate::output::presenter::Presenter;

pub fn run() -> i32 {
    let cli = Cli::parse();

    // Init tracing
    let level = cli.log_level.as_deref().unwrap_or("info");
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(level)
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

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

    if cli.clean_before_execution {
        match crate::support::temp::platform_logs_dir(&config.work_path)
            .and_then(|dir| crate::support::fs::clean_dir(&dir))
        {
            Ok(()) => {}
            Err(e) => {
                presenter.print_error(&format!("failed to clean platform logs: {e}"));
                return crate::output::exit_codes::RUNTIME_ERROR;
            }
        }
    }

    let result = match &cli.command {
        Command::Build(args) => crate::use_cases::build_project::execute(&config, args, &presenter),
        Command::Test(args) => crate::use_cases::run_tests::execute(&config, args, &presenter),
        Command::Dump(args) => crate::use_cases::dump_config::execute(&config, args, &presenter),
        Command::Syntax(args) => crate::use_cases::check_syntax::execute(&config, args, &presenter),
        Command::Launch(args) => crate::use_cases::launch_app::execute(&config, args, &presenter),
    };

    match result {
        Ok(()) => 0,
        Err(e) => {
            error!("{e}");
            e.exit_code()
        }
    }
}
