use crate::{error::Result, sys::shell, ui::*};
use colored::Colorize;

pub fn register() -> Result<()> {
    print_section("Registering Shell Association");
    let spinner = Spinner::new("Registering .mfz file association in Windows Shell…");
    shell::register_file_association()?;
    spinner.success("Successfully registered .mfz file extension in HKCU\\Software\\Classes.");
    println!();
    println!(
        "    {} Double-clicking any .mfz file will now launch MFZ automatically.",
        "✓".green().bold()
    );
    println!(
        "    {} You can export desktop shortcuts with: {}",
        "TIP:".cyan().bold(),
        "mfz export --all --desktop".yellow()
    );
    Ok(())
}

pub fn unregister() -> Result<()> {
    print_section("Unregistering Shell Association");
    let spinner = Spinner::new("Removing .mfz file association from Windows Shell…");
    shell::unregister_file_association()?;
    spinner.success("Successfully unregistered .mfz file extension.");
    Ok(())
}
