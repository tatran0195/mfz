use colored::Colorize;
use console::Term;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

// ─── Spinner ──────────────────────────────────────────────────────────────────

pub struct Spinner {
    pb: ProgressBar,
}

impl Spinner {
    pub fn new(msg: impl Into<String>) -> Self {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner:.cyan.bold} {msg}")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        pb.set_message(msg.into());
        pb.enable_steady_tick(Duration::from_millis(80));
        Self { pb }
    }

    pub fn set_message(&self, msg: impl Into<String>) {
        self.pb.set_message(msg.into());
    }

    pub fn success(self, msg: impl Into<String>) {
        self.pb.finish_with_message(format!(
            "{} {}",
            "✔".green().bold(),
            msg.into().green()
        ));
    }

    pub fn failure(self, msg: impl Into<String>) {
        self.pb.finish_with_message(format!(
            "{} {}",
            "✘".red().bold(),
            msg.into().red()
        ));
    }

    pub fn warning(self, msg: impl Into<String>) {
        self.pb.finish_with_message(format!(
            "{} {}",
            "⚠".yellow().bold(),
            msg.into().yellow()
        ));
    }
}

// ─── Print helpers ────────────────────────────────────────────────────────────

pub fn print_banner() {
    if console::user_attended() {
        let _ = Term::stdout().clear_screen();
    }
    println!(
        "{}",
        concat!(
            "\n",
            " ███╗   ███╗███████╗███████╗\n",
            " ████╗ ████║██╔════╝╚══███╔╝\n",
            " ██╔████╔██║█████╗    ███╔╝ \n",
            " ██║╚██╔╝██║██╔══╝   ███╔╝  \n",
            " ██║ ╚═╝ ██║██║     ███████╗\n",
            " ╚═╝     ╚═╝╚═╝     ╚══════╝\n"
        )
        .cyan()
        .bold()
    );
    println!(
        "  {} {}\n",
        "AWS MFA + SSM Port Forwarding + RDP Session Manager".white(),
        "v0.1.0".dimmed()
    );
    print_separator();
    println!();
}

pub fn print_section(title: &str) {
    println!("\n  {} {}", "▶".cyan().bold(), title.white().bold());
    println!("  {}", "─".repeat(52).dimmed());
}

pub fn print_kv(label: &str, value: &str) {
    println!(
        "    {} {:<24} {}",
        "•".dimmed(),
        label.dimmed(),
        value.white()
    );
}

pub fn print_success(msg: &str) {
    println!("  {} {}", "✔".green().bold(), msg.green());
}

pub fn print_error(msg: &str) {
    println!("  {} {}", "✘".red().bold(), msg.red());
}

pub fn print_warning(msg: &str) {
    println!("  {} {}", "⚠".yellow().bold(), msg.yellow());
}

pub fn print_info(msg: &str) {
    println!("  {} {}", "ℹ".cyan(), msg.white());
}

pub fn print_separator() {
    println!("{}", "─".repeat(90).dimmed());
}