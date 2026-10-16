//! Local QR/OTP operations. IAM enrollment lives in `enrollment`, so image and
//! URI parsing can be exercised without a cloud account or generated EC2 SDK.
use std::{
    fs::File,
    io::{BufReader, Cursor},
    path::Path,
};

use base32::{encode as b32encode, Alphabet};
use image::{GrayImage, Luma};
use qrcode::QrCode;
use rand::{rngs::OsRng, RngCore};
use rxing::Reader;
use url::Url;

use super::otp;
use crate::{
    error::{AppError, Result},
    sys::storage,
    ui::print_success,
    validation,
};

#[derive(Clone)]
pub struct OtpSecret {
    pub uri: String,
    pub secret: String,
}

/// Decode a bounded local image, then parse an actual TOTP provisioning URI.
pub fn decode(image_path: &str) -> Result<OtpSecret> {
    const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
    if !std::fs::metadata(image_path)?.is_file() {
        return Err(AppError::QrCode("Image must be a regular file".into()));
    }
    let file = File::open(image_path)?;
    if file.metadata()?.len() > MAX_IMAGE_BYTES {
        return Err(AppError::QrCode(
            "Image exceeds the 16 MiB input limit".into(),
        ));
    }
    let mut reader = image::ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|error| AppError::QrCode(format!("Cannot identify image format: {error}")))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|error| AppError::QrCode(format!("Cannot decode image: {error}")))?
        .to_luma8();
    let (width, height) = img.dimensions();
    let source = rxing::Luma8LuminanceSource::new(img.into_raw(), width, height);
    let mut bitmap = rxing::BinaryBitmap::new(rxing::common::HybridBinarizer::new(source));
    let result = rxing::qrcode::QRCodeReader
        .decode(&mut bitmap)
        .map_err(|error| AppError::QrCode(format!("QR decode failed: {error}")))?;
    extract_secret_from_uri(result.getText())
}

pub fn extract_secret_from_uri(uri: &str) -> Result<OtpSecret> {
    let parsed =
        Url::parse(uri).map_err(|_| AppError::QrCode("Invalid TOTP provisioning URI".into()))?;
    if parsed.scheme() != "otpauth"
        || parsed.host_str() != Some("totp")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.fragment().is_some()
        || parsed.path().trim_matches('/').is_empty()
    {
        return Err(AppError::QrCode(
            "Expected otpauth://totp/<label>, without userinfo, port or fragment".into(),
        ));
    }
    let mut secret = None;
    for (key, value) in parsed.query_pairs() {
        if key == "secret" && secret.replace(value.into_owned()).is_some() {
            return Err(AppError::QrCode(
                "TOTP URI contains multiple secret parameters".into(),
            ));
        }
    }
    let secret = secret
        .ok_or_else(|| AppError::QrCode("TOTP URI is missing its secret parameter".into()))?;
    let secret = b32encode(
        Alphabet::Rfc4648 { padding: false },
        &otp::decode_secret(&secret).map_err(|_| {
            AppError::QrCode("TOTP URI secret is not valid, nonempty Base-32".into())
        })?,
    );
    // This command extracts the key; it does not change a URI's algorithm,
    // digits or period. MFZ's IAM MFA authentication uses AWS's SHA1/6/30 format.
    Ok(OtpSecret {
        uri: parsed.to_string(),
        secret,
    })
}

#[derive(Clone)]
pub struct GeneratedQr {
    pub secret_b32: String,
    pub uri: String,
    pub png_bytes: Vec<u8>,
}

pub fn generate(secret_b32: Option<&str>, account: &str, issuer: &str) -> Result<GeneratedQr> {
    for (label, value) in [("Account", account), ("Issuer", issuer)] {
        validation::single_line(value, label)?;
        if value.trim().is_empty() || value.contains(':') {
            return Err(AppError::QrCode(format!(
                "{label} must not be empty or contain a colon (TOTP label separator)"
            )));
        }
    }
    let raw = match secret_b32 {
        Some(secret) => otp::decode_secret(secret).map_err(|_| {
            AppError::QrCode("Provided secret is not valid, nonempty Base-32".into())
        })?,
        None => {
            let mut raw = [0u8; 20];
            OsRng
                .try_fill_bytes(&mut raw)
                .map_err(|_| AppError::QrCode("System randomness is unavailable".into()))?;
            raw.to_vec()
        }
    };
    let secret_b32 = b32encode(Alphabet::Rfc4648 { padding: false }, &raw);
    let mut uri = Url::parse("otpauth://totp/").expect("fixed provisioning URL is valid");
    uri.path_segments_mut()
        .map_err(|_| AppError::QrCode("Cannot construct TOTP label".into()))?
        .push(&format!("{issuer}:{account}"));
    uri.query_pairs_mut()
        .append_pair("secret", &secret_b32)
        .append_pair("issuer", issuer)
        .append_pair("algorithm", "SHA1")
        .append_pair("digits", "6")
        .append_pair("period", "30");
    let uri = uri.to_string();
    let code = QrCode::new(uri.as_bytes())
        .map_err(|error| AppError::QrCode(format!("QR encode failed: {error}")))?;
    let module_size = 8u32;
    let quiet = 4 * module_size;
    let size = code.width() as u32 * module_size + quiet * 2;
    let mut img = GrayImage::from_pixel(size, size, Luma([255]));
    for y in 0..code.width() {
        for x in 0..code.width() {
            if code[(x, y)] != qrcode::types::Color::Dark {
                continue;
            }
            for dy in 0..module_size {
                for dx in 0..module_size {
                    img.put_pixel(
                        x as u32 * module_size + quiet + dx,
                        y as u32 * module_size + quiet + dy,
                        Luma([0]),
                    );
                }
            }
        }
    }
    let mut png_bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .map_err(|error| AppError::QrCode(format!("PNG encoding failed: {error}")))?;
    Ok(GeneratedQr {
        secret_b32,
        uri,
        png_bytes,
    })
}

pub fn save_png(qr: &GeneratedQr, path: &str) -> Result<()> {
    save_png_with_overwrite(qr, path, false)
}

pub fn save_png_with_overwrite(qr: &GeneratedQr, path: &str, overwrite: bool) -> Result<()> {
    storage::atomic_write(Path::new(path), &qr.png_bytes, overwrite, true)?;
    print_success(&format!(
        "QR image saved → {path} (contains the authenticator secret; keep private)"
    ));
    Ok(())
}

pub fn print_terminal(qr: &GeneratedQr) {
    println!();
    if let Err(error) = qr2term::print_qr(&qr.uri) {
        println!("Could not render inline QR: {error}");
    }
    println!();
}

impl std::fmt::Debug for OtpSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OtpSecret")
            .field("uri", &"[REDACTED]")
            .field("secret", &"[REDACTED]")
            .finish()
    }
}
impl std::fmt::Debug for GeneratedQr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedQr")
            .field("uri", &"[REDACTED]")
            .field("secret_b32", &"[REDACTED]")
            .field(
                "png_bytes",
                &format_args!("[{} secret-bearing bytes]", self.png_bytes.len()),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_uri_parsing_decodes_padding_and_rejects_wrong_schemes_and_duplicate_secrets() {
        assert_eq!(
            extract_secret_from_uri("otpauth://totp/label?secret=my%3D%3D%3D%3D%3D%3D")
                .unwrap()
                .secret,
            "MY"
        );
        for uri in [
            "https://example.org/?secret=MY",
            "otpauth://hotp/label?secret=MY",
            "otpauth://totp/?secret=MY",
            "otpauth://totp/label?secret=",
            "otpauth://totp/label?secret=invalid-key",
            "otpauth://totp/label?secret=MY&secret=MZXQ",
            "otpauth://user@totp/label?secret=MY",
        ] {
            assert!(
                extract_secret_from_uri(uri).is_err(),
                "unexpectedly accepted {uri}"
            );
        }
    }

    #[test]
    fn special_labels_are_encoded_and_the_actual_png_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("qr.png");
        let issuer = "R&D / Nhóm";
        let generated = generate(
            Some("jbswy3dpehpk3pxp"),
            "A+B & C/account@example.org",
            issuer,
        )
        .unwrap();
        let uri = Url::parse(&generated.uri).unwrap();
        assert_eq!(
            uri.query_pairs()
                .find(|(key, _)| key == "issuer")
                .unwrap()
                .1,
            issuer
        );
        assert_eq!(uri.path_segments().unwrap().count(), 1);
        save_png(&generated, path.to_str().unwrap()).unwrap();
        let decoded = decode(path.to_str().unwrap()).unwrap();
        assert_eq!(decoded.secret, "JBSWY3DPEHPK3PXP");
        assert_eq!(decoded.uri, generated.uri);
        assert!(!format!("{generated:?}").contains(&generated.secret_b32));
        assert!(!format!("{decoded:?}").contains(&generated.secret_b32));
    }

    #[test]
    fn png_save_is_no_clobber_unless_overwrite_is_explicit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("important.png");
        std::fs::write(&path, b"original user data").unwrap();
        let generated = generate(Some("JBSWY3DPEHPK3PXP"), "user", "AWS").unwrap();
        assert!(save_png(&generated, path.to_str().unwrap()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original user data");
        save_png_with_overwrite(&generated, path.to_str().unwrap(), true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), generated.png_bytes);
    }

    #[test]
    fn compressed_image_dimensions_and_input_size_are_bounded_before_qr_scanning() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("too-wide.png");
        GrayImage::new(5000, 1).save(&path).unwrap();
        let error = decode(path.to_str().unwrap()).unwrap_err().to_string();
        assert!(
            error.contains("Cannot decode image") && error.to_lowercase().contains("limit"),
            "{error}"
        );
        let large = directory.path().join("too-large.png");
        File::create(&large)
            .unwrap()
            .set_len(16 * 1024 * 1024 + 1)
            .unwrap();
        assert!(decode(large.to_str().unwrap())
            .unwrap_err()
            .to_string()
            .contains("16 MiB"));
    }
}

#[cfg(test)]
mod independent_uri_contract {
    #[test]
    fn generated_uri_is_understood_by_an_independent_totp_implementation() {
        let key = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        let generated = super::generate(Some(key), "alice+dev/example.org", "R&D / Nhóm").unwrap();
        let parsed = totp_rs::TOTP::from_url(&generated.uri).unwrap();
        assert_eq!(parsed.algorithm, totp_rs::Algorithm::SHA1);
        assert_eq!(parsed.digits, 6);
        assert_eq!(parsed.step, 30);
        assert_eq!(parsed.issuer.as_deref(), Some("R&D / Nhóm"));
        assert_eq!(parsed.account_name, "alice+dev/example.org");
        assert_eq!(parsed.get_secret_base32(), key);
        assert!(super::generate(Some(key), "account:ambiguous", "AWS").is_err());
    }
}
