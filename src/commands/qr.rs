use crate::{
    auth::{
        enrollment::{self, RegistrationOptions},
        qr,
    },
    cli::QrCommand,
    config::ConfigLoader,
    error::Result,
    ui::*,
};
use colored::Colorize;

pub async fn run(sub: QrCommand) -> Result<()> {
    match sub {
        QrCommand::Decode { image } => {
            print_section("QR Code Decoder");
            let spinner = Spinner::new(format!("Scanning `{image}`…"));
            let secret = match qr::decode(&image) {
                Ok(secret) => {
                    spinner.success("Decoded successfully");
                    secret
                }
                Err(error) => {
                    spinner.failure(error.to_string());
                    return Err(error);
                }
            };
            println!();
            print_kv("OTP URI", &secret.uri);
            println!();
            println!(
                "  {}  {}",
                "SECRET".cyan().bold(),
                secret.secret.green().bold()
            );
            println!();
        }
        QrCommand::Generate(args) => {
            print_section("QR Code Generator");
            let spinner = Spinner::new("Generating TOTP secret and QR code…");
            let generated = match qr::generate(args.secret.as_deref(), &args.account, &args.issuer)
            {
                Ok(generated) => {
                    spinner.success("QR code generated");
                    generated
                }
                Err(error) => {
                    spinner.failure(error.to_string());
                    return Err(error);
                }
            };

            qr::print_terminal(&generated);
            print_kv("Account", &args.account);
            print_kv("Issuer", &args.issuer);
            println!();
            println!(
                "  {}  {}",
                "SECRET (Base-32)".cyan().bold(),
                generated.secret_b32.green().bold()
            );
            println!();
            print_kv("OTP URI", &generated.uri);
            println!();

            if let Some(ref path) = args.out {
                qr::save_png_with_overwrite(&generated, path, args.force)?;
            } else {
                print_info("Tip: pass --out totp.png to also save a scannable PNG image");
            }
        }
        QrCommand::Register(args) => {
            let account = args
                .account
                .clone()
                .unwrap_or_else(|| args.username.clone());
            let config = ConfigLoader::from_cli(args.config.as_deref());
            enrollment::register(RegistrationOptions {
                profile: &args.profile,
                username: &args.username,
                account: &account,
                issuer: &args.issuer,
                out_png: args.out.as_deref(),
                overwrite_png: args.force,
                region: args.region.as_deref(),
                save_env: args.save.as_deref(),
                config: config.path(),
            })
            .await?;
        }
    }

    Ok(())
}
