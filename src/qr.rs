//! Three capabilities:
//!   1. `decode`    — read a QR image → extract OTP secret
//!   2. `generate`  — build a TOTP URI → render QR PNG + terminal preview
//!   3. `register`  — generate + call AWS IAM CreateVirtualMFADevice /
//!                    EnableMFADevice in one flow

use std::io::Write;

use aws_config::BehaviorVersion;
use aws_sdk_iam::Client as IamClient;
use base32::{decode as b32decode, encode as b32encode, Alphabet};
use colored::Colorize;
use image::{GrayImage, Luma};
use qrcode::QrCode;
use rand::{thread_rng, RngCore};
use regex::Regex;
use rxing::Reader;
use tracing::info;

use crate::{
    error::{AppError, Result},
    ui::{print_info, print_kv, print_section, print_separator, print_success, Spinner},
};

// ─── Decode ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OtpSecret {
    pub uri: String,
    pub secret: String,
}

/// Decode the first QR code in `image_path` and extract the TOTP secret.
pub fn decode(image_path: &str) -> Result<OtpSecret> {
    let img = image::open(image_path)
        .map_err(|e| AppError::QrCode(format!("Cannot open image `{image_path}`: {e}")))?
        .to_luma8();

    let (width, height) = img.dimensions();
    let source = rxing::Luma8LuminanceSource::new(img.into_raw(), width, height);
    let binarizer = rxing::common::HybridBinarizer::new(source);
    let mut bitmap = rxing::BinaryBitmap::new(binarizer);
    let mut reader = rxing::qrcode::QRCodeReader::default();

    let result = reader
        .decode(&mut bitmap)
        .map_err(|e| AppError::QrCode(format!("QR decode failed: {e}")))?;

    let uri = result.getText().to_string();

    extract_secret_from_uri(&uri)
}

fn extract_secret_from_uri(uri: &str) -> Result<OtpSecret> {
    let re =
        Regex::new(r"secret=([^&]+)").map_err(|e| AppError::QrCode(format!("Regex error: {e}")))?;

    let secret = re
        .captures(uri)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned())
        .ok_or_else(|| AppError::QrCode("No `secret=…` parameter in OTP URI".into()))?;

    Ok(OtpSecret {
        uri: uri.to_owned(),
        secret,
    })
}

// ─── Generate ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GeneratedQr {
    /// Base-32 encoded secret (what goes into the authenticator app)
    pub secret_b32: String,
    /// Full otpauth:// URI
    pub uri: String,
    /// PNG bytes
    pub png_bytes: Vec<u8>,
}

/// Build a TOTP QR code.
///
/// * `secret_b32` — supply an existing Base-32 secret, or `None` to generate a fresh 20-byte one.
/// * `account`    — label shown in the authenticator (e.g. "alice@example.com").
/// * `issuer`     — issuer prefix (e.g. "AWS", "MyOrg").
pub fn generate(secret_b32: Option<&str>, account: &str, issuer: &str) -> Result<GeneratedQr> {
    // ── Secret ────────────────────────────────────────────────────────────
    let secret_b32 = match secret_b32 {
        Some(s) => {
            // Validate by attempting a decode
            b32decode(Alphabet::Rfc4648 { padding: false }, s)
                .ok_or_else(|| AppError::QrCode("Provided secret is not valid Base-32".into()))?;
            s.to_uppercase()
        }
        None => {
            let mut raw = [0u8; 20]; // 160-bit secret
            thread_rng().fill_bytes(&mut raw);
            b32encode(Alphabet::Rfc4648 { padding: false }, &raw).to_uppercase()
        }
    };

    // ── OTP URI ───────────────────────────────────────────────────────────
    // Format: otpauth://totp/<issuer>:<account>?secret=<B32>&issuer=<issuer>
    let uri = format!(
        "otpauth://totp/{issuer}:{account}?secret={secret_b32}&issuer={issuer}&algorithm=SHA1&digits=6&period=30"
    );

    // ── QR PNG ────────────────────────────────────────────────────────────
    let code = QrCode::new(uri.as_bytes())
        .map_err(|e| AppError::QrCode(format!("QR encode failed: {e}")))?;

    // Render to a 1-pixel-per-module greyscale image then upscale ×8
    let module_size = 8u32;
    let width_modules = code.width() as u32;
    let quiet = 4 * module_size; // 4-module quiet zone on each side
    let img_size = width_modules * module_size + quiet * 2;

    let mut img = GrayImage::from_pixel(img_size, img_size, Luma([255u8]));

    let width = code.width();
    for y_mod in 0..width {
        for x_mod in 0..width {
            let color = code[(x_mod, y_mod)];
            let val: u8 = if color == qrcode::types::Color::Dark {
                0
            } else {
                255
            };
            let luma = Luma([val]);
            let x0 = x_mod as u32 * module_size + quiet;
            let y0 = y_mod as u32 * module_size + quiet;
            for dy in 0..module_size {
                for dx in 0..module_size {
                    img.put_pixel(x0 + dx, y0 + dy, luma);
                }
            }
        }
    }

    let mut png_bytes: Vec<u8> = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .map_err(|e| AppError::QrCode(format!("PNG encoding failed: {e}")))?;

    Ok(GeneratedQr {
        secret_b32,
        uri,
        png_bytes,
    })
}

/// Save a `GeneratedQr`'s PNG to disk and print confirmation.
pub fn save_png(qr: &GeneratedQr, path: &str) -> Result<()> {
    std::fs::write(path, &qr.png_bytes)
        .map_err(|e| AppError::QrCode(format!("Cannot write PNG to `{path}`: {e}")))?;
    print_success(&format!("QR image saved → {path}"));
    Ok(())
}

/// Render the QR code as Unicode half-blocks directly in the terminal.
pub fn print_terminal(qr: &GeneratedQr) {
    println!();
    // qr2term accepts the raw string content
    if let Err(e) = qr2term::print_qr(&qr.uri) {
        // Non-fatal: just warn
        println!("  (Could not render inline QR: {e})");
    }
    println!();
}

// ─── Register ─────────────────────────────────────────────────────────────────

/// Full registration flow:
///   1. CreateVirtualMFADevice   → get serial ARN + secret seed
///   2. Show QR in terminal (and optionally save PNG)
///   3. Prompt user for two consecutive TOTP codes
///   4. EnableMFADevice          → activate the device on the IAM user
pub async fn register(
    profile: &str,
    username: &str,
    account: &str,
    issuer: &str,
    out_png: Option<&str>,
    region: Option<&str>,
    save_env: Option<&str>,
) -> Result<()> {
    // ── 1. Build AWS IAM client ───────────────────────────────────────────
    let mut builder = aws_config::defaults(BehaviorVersion::latest()).profile_name(profile);
    if let Some(r) = region {
        builder = builder.region(aws_config::Region::new(r.to_owned()));
    }
    let config = builder.load().await;
    let iam = IamClient::new(&config);

    // ── 2. CreateVirtualMFADevice ─────────────────────────────────────────
    print_section("Creating Virtual MFA Device");
    let device_name = format!("{username}-totp");

    let spinner = Spinner::new(format!("Creating IAM virtual MFA device `{device_name}`…"));
    let create_resp = iam
        .create_virtual_mfa_device()
        .virtual_mfa_device_name(&device_name)
        .send()
        .await
        .map_err(|e| AppError::Iam(format!("CreateVirtualMFADevice failed: {e}")))?;

    let device = create_resp
        .virtual_mfa_device()
        .ok_or_else(|| AppError::Iam("No device in CreateVirtualMFADevice response".into()))?;

    let serial_number = device.serial_number().to_owned();

    // AWS returns the raw Base-32 secret seed (not a URI).
    spinner.set_message("Parsing MFA secret seed...");
    let seed_b32 = device
        .base32_string_seed()
        .ok_or_else(|| AppError::Iam("No Base32 seed in response".into()))
        .and_then(|blob| {
            std::str::from_utf8(blob.as_ref())
                .map(|s| s.to_uppercase())
                .map_err(|_| AppError::Iam("Seed bytes are not valid UTF-8".into()))
        })?;

    spinner.set_message("Linking device to authenticator app...");

    spinner.success(format!("Device created: {serial_number}"));
    info!("Virtual MFA device ARN: {}", serial_number);

    // ── 3. Build + display QR ─────────────────────────────────────────────
    print_section("Scanning QR Code");

    let qr = generate(Some(&seed_b32), account, issuer)?;
    print_terminal(&qr);

    print_kv("Secret (Base-32)", &qr.secret_b32);
    print_kv("Serial", &serial_number);
    println!();

    if let Some(path) = out_png {
        save_png(&qr, path)?;
    }

    print_info("Scan the QR code above with your authenticator app.");
    print_info("Then enter two consecutive 6-digit codes to activate the device.");
    println!();

    // ── 4. Prompt for two TOTP codes ──────────────────────────────────────
    let code1 = prompt_totp("Enter TOTP code 1")?;
    let code2 = prompt_totp("Enter TOTP code 2")?;

    // ── 5. EnableMFADevice ────────────────────────────────────────────────
    print_section("Activating MFA Device");

    let spinner = Spinner::new("Calling IAM EnableMFADevice…");
    iam.enable_mfa_device()
        .user_name(username)
        .serial_number(&serial_number)
        .authentication_code1(&code1)
        .authentication_code2(&code2)
        .send()
        .await
        .map_err(|e| AppError::Iam(format!("EnableMFADevice failed (wrong codes?): {e}")))?;

    spinner.success("MFA device activated successfully!");

    // ── 6. Summary ────────────────────────────────────────────────────────
    print_separator();
    print_section("Summary — save these values in mfz.ini");
    print_kv("SERIAL", &serial_number);
    print_kv("SECRET", &qr.secret_b32);
    println!();

    if let Some(target) = save_env {
        let spinner = Spinner::new(format!("Saving to mfz.ini under `[{target}]`…"));
        let loader = crate::config::ConfigLoader::new("mfz.ini");

        let mut new_config = if let Ok(existing) = loader.load(target) {
            existing
        } else {
            crate::config::EnvironmentConfig {
                profile: profile.to_owned(),
                serial: serial_number.clone(),
                secret: qr.secret_b32.clone(),
                instance: Some("REPLACE_ME".into()),
                instance_tag: None,
                remote_port: 3389,
                local_port: 3389,
                rdp_host: "REPLACE_ME".into(),
                rdp_user: None,
                rdp_password: None,
            }
        };

        new_config.serial = serial_number;
        new_config.secret = qr.secret_b32;

        if let Err(e) = loader.save(target, &new_config) {
            spinner.failure(format!("Failed to save config: {e}"));
        } else {
            spinner.success(format!("Credentials saved to `[{target}]` section."));
        }
    }

    print_success("Registration complete. Your authenticator is now linked.");
    println!();

    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Read a 6-digit TOTP code from stdin, retrying on bad input.
fn prompt_totp(prompt: &str) -> Result<String> {
    loop {
        print!("  {} {}: ", "›".cyan().bold(), prompt.white().bold());
        std::io::stdout().flush().map_err(|e| AppError::Io(e))?;

        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .map_err(|e| AppError::Io(e))?;

        let code = input.trim().to_owned();

        if code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()) {
            return Ok(code);
        }

        println!(
            "  {} Code must be exactly 6 digits — please try again.",
            "⚠".yellow().bold()
        );
    }
}
