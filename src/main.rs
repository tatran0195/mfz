use clap::Parser;
use colored::Colorize;
use error::Result;
use mfz::{
    cli::{Cli, TopCommand},
    commands, error, ui,
};
use tracing_appender::rolling;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use ui::*;

// ─── Logging ──────────────────────────────────────────────────────────────────

fn setup_logging(enable_file: bool) -> Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let mut guard = None;
    let mut layers = Vec::new();

    // Console layer (always on)
    let console_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .with_target(false)
        .with_filter(EnvFilter::new("mfz=info,warn"))
        .boxed();
    layers.push(console_layer);

    // File layer (optional)
    if enable_file {
        let file_appender = rolling::Builder::new()
            .rotation(rolling::Rotation::DAILY)
            .filename_prefix("mfz.log")
            .build(".")
            .map_err(|error| {
                error::AppError::Config(format!("Cannot initialize file logging: {error}"))
            })?;
        let (non_blocking, worker_guard) = tracing_appender::non_blocking(file_appender);
        guard = Some(worker_guard);

        let file_layer = fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_target(false)
            .boxed();
        layers.push(file_layer);
    }

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(layers)
        .try_init()
        .map_err(|error| error::AppError::Config(format!("Cannot initialize logging: {error}")))?;
    Ok(guard)
}

// ─── Entry ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    {
        // Enable ANSI support
        colored::control::set_virtual_terminal(true).ok();
        // Set UTF-8 code page natively — no `cmd /C chcp 65001` subprocess needed
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCP(65001);
            windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
        }
        // Attempt to set a modern font (Consolas) for better symbol rendering
        setup_console_font();
    }

    let cli = Cli::parse();
    let _log_guard = match setup_logging(cli.log) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::from(error.exit_code() as u8);
        }
    };
    let is_mfz_launch = matches!(&cli.command, TopCommand::Launch(args)
        if args.environment.as_ref().is_some_and(|path| path.to_ascii_lowercase().ends_with(".mfz")));
    print_banner();

    if let Err(e) = dispatch(cli).await {
        tracing::error!("{e}");
        print_error(&e.to_string());
        // If launched via .mfz file association, pause so user can inspect any error
        if is_mfz_launch && console::user_attended() {
            println!("\n  {} Press Enter to exit...", "ℹ".cyan());
            let mut buf = String::new();
            let _ = std::io::stdin().read_line(&mut buf);
        }
        return std::process::ExitCode::from(e.exit_code() as u8);
    }
    std::process::ExitCode::SUCCESS
}

async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        TopCommand::Launch(args) => commands::launch::run(args).await,
        TopCommand::List(args) => commands::list::run(args),
        TopCommand::Hosts(sub) => commands::hosts::run(sub),
        TopCommand::Qr(sub) => commands::qr::run(sub).await,
        TopCommand::Config(sub) => commands::config::run(*sub).await,
        TopCommand::Register => commands::shell::register(),
        TopCommand::Unregister => commands::shell::unregister(),
        TopCommand::Export(args) => commands::export::run(args).await,
    }
}

#[cfg(windows)]
fn setup_console_font() {
    use windows_sys::Win32::System::Console::{
        GetCurrentConsoleFontEx, GetStdHandle, SetCurrentConsoleFontEx, CONSOLE_FONT_INFOEX, COORD,
        STD_OUTPUT_HANDLE,
    };

    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle == 0 || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return;
        }

        let mut font_info = CONSOLE_FONT_INFOEX {
            cbSize: std::mem::size_of::<CONSOLE_FONT_INFOEX>() as u32,
            nFont: 0,
            dwFontSize: COORD { X: 0, Y: 18 },
            FontFamily: 54, // FF_MODERN
            FontWeight: 400,
            FaceName: [0; 32],
        };

        // Query current font settings (optional pre-flight check)
        let mut current_font = CONSOLE_FONT_INFOEX {
            cbSize: std::mem::size_of::<CONSOLE_FONT_INFOEX>() as u32,
            nFont: 0,
            dwFontSize: COORD { X: 0, Y: 0 },
            FontFamily: 0,
            FontWeight: 0,
            FaceName: [0; 32],
        };

        let _ = GetCurrentConsoleFontEx(handle, 0, &mut current_font);

        let font_name = "Consolas";
        for (i, c) in font_name.encode_utf16().enumerate() {
            if i < 31 {
                font_info.FaceName[i] = c;
            }
        }

        SetCurrentConsoleFontEx(handle, 0, &font_info);
    }
}
