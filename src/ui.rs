use std::time::Duration;

use colored::Colorize;
use console::{Emoji, Term};
use indicatif::{ProgressBar, ProgressStyle};

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
                .tick_strings(&["·", "o", "O", "o", "·", " "]),
        );
        pb.set_message(msg.into());
        pb.enable_steady_tick(Duration::from_millis(80));
        Self { pb }
    }

    pub fn set_message(&self, msg: impl Into<String>) {
        self.pb.set_message(msg.into());
    }

    pub fn success(self, msg: impl Into<String>) {
        let symbol = Emoji("√", "OK");
        self.pb.finish_with_message(format!(
            "{} {}",
            symbol.to_string().green().bold(),
            msg.into().green()
        ));
    }

    pub fn failure(self, msg: impl Into<String>) {
        let symbol = Emoji("×", "X");
        self.pb.finish_with_message(format!(
            "{} {}",
            symbol.to_string().red().bold(),
            msg.into().red()
        ));
    }

    pub fn warning(self, msg: impl Into<String>) {
        let symbol = Emoji("!", "!");
        self.pb.finish_with_message(format!(
            "{} {}",
            symbol.to_string().yellow().bold(),
            msg.into().yellow()
        ));
    }

    pub fn failure_no_pb(msg: impl Into<String>) {
        let symbol = Emoji("×", "X");
        println!("{} {}", symbol.to_string().red().bold(), msg.into().red());
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
    let symbol = Emoji("»", ">");
    println!(
        "\n  {} {}",
        symbol.to_string().cyan().bold(),
        title.white().bold()
    );
    println!("  {}", "─".repeat(52).dimmed());
}

pub fn print_kv(label: &str, value: &str) {
    let symbol = Emoji("•", "-");
    println!(
        "    {} {:<24} {}",
        symbol.to_string().dimmed(),
        label.dimmed(),
        value.white()
    );
}

pub fn print_success(msg: &str) {
    let symbol = Emoji("√", "OK");
    println!("  {} {}", symbol.to_string().green().bold(), msg.green());
}

pub fn print_error(msg: &str) {
    let symbol = Emoji("×", "X");
    println!("  {} {}", symbol.to_string().red().bold(), msg.red());
}

pub fn print_warning(msg: &str) {
    let symbol = Emoji("!", "!");
    println!("  {} {}", symbol.to_string().yellow().bold(), msg.yellow());
}

pub fn print_info(msg: &str) {
    let symbol = Emoji("i", "i");
    println!("  {} {}", symbol.to_string().cyan(), msg.white());
}

pub fn print_separator() {
    println!("{}", "─".repeat(90).dimmed());
}
