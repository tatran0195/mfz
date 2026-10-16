//! Validation for values crossing line-oriented configuration/Windows boundaries.
use crate::error::{AppError, Result};

pub(crate) fn single_line(value: &str, label: &str) -> Result<()> {
    if value.chars().any(char::is_control) {
        return Err(AppError::Config(format!(
            "{label} must not contain control characters"
        )));
    }
    Ok(())
}

pub(crate) fn environment_name(name: &str) -> Result<String> {
    if name.is_empty()
        || name.len() > 128
        || name.trim() != name
        || matches!(name, "." | "..")
        || name.ends_with('.')
        || name
            .chars()
            .any(|c| c.is_control() || "[]/\\:=*?\"<>|".contains(c))
    {
        return Err(AppError::Config(
            "Invalid environment name (must also be a safe profile filename)".into(),
        ));
    }
    let lower = name.to_lowercase();
    let stem = lower
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ');
    let device = matches!(stem, "con" | "prn" | "aux" | "nul" | "conin$" | "conout$")
        || stem
            .strip_prefix("com")
            .or_else(|| stem.strip_prefix("lpt"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            });
    if device {
        return Err(AppError::Config(
            "Environment name is a reserved Windows device name".into(),
        ));
    }
    Ok(lower)
}

pub(crate) fn hostname(host: &str) -> Result<()> {
    if host.is_empty()
        || host.len() > 253
        || host
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_' | '.'))
        || host.starts_with('.')
        || host.ends_with('.')
        || host
            .split('.')
            .any(|label| label.is_empty() || label.len() > 63)
    {
        return Err(AppError::Config(
            "Invalid host alias; use a hostname or IPv4 address, without a port or whitespace"
                .into(),
        ));
    }
    Ok(())
}

pub(crate) fn optimization(name: &str) -> Result<()> {
    match name.trim().to_ascii_lowercase().as_str() {
        "none" | "speed" | "fast" | "lan" | "balanced" | "balance" | "quality" | "high"
        | "best" => Ok(()),
        _ => Err(AppError::Config(
            "Unknown optimization profile; use speed, balanced, quality or none".into(),
        )),
    }
}

/// Normalize both documented CLI separators; never pass arbitrary RDP lines on.
pub(crate) fn drives(value: &str) -> Result<Option<String>> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    if value == "*" {
        return Ok(Some("*".into()));
    }
    let mut result = Vec::new();
    for part in value.split([';', ',']) {
        let part = part.trim();
        let bytes = part.as_bytes();
        if bytes.len() != 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
            return Err(AppError::Config(
                "Shared drives must be *, none, or drive letters such as C:;D:".into(),
            ));
        }
        let drive = format!("{}:", char::from(bytes[0].to_ascii_uppercase()));
        if !result.contains(&drive) {
            result.push(drive);
        }
    }
    Ok(Some(result.join(";")))
}

/// Microsoft CRT/CommandLineToArgvW quoting, including backslashes immediately
/// before quotes and at the end of a quoted argument. Not shell quoting.
#[cfg(any(windows, test))]
pub(crate) fn windows_command_line(args: &[&str]) -> Result<String> {
    let mut output = Vec::with_capacity(args.len());
    for argument in args {
        if argument.contains('\0') {
            return Err(AppError::Config(
                "NUL is not a valid Windows command argument".into(),
            ));
        }
        let mut quoted = String::from("\"");
        let mut backslashes = 0;
        for ch in argument.chars() {
            match ch {
                '\\' => backslashes += 1,
                '\"' => {
                    quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    quoted.push('\"');
                    backslashes = 0;
                }
                _ => {
                    quoted.extend(std::iter::repeat_n('\\', backslashes));
                    backslashes = 0;
                    quoted.push(ch);
                }
            }
        }
        quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
        quoted.push('\"');
        output.push(quoted);
    }
    Ok(output.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_argument_quoting_preserves_spaces_quotes_and_trailing_slashes() {
        assert_eq!(windows_command_line(&[""]).unwrap(), "\"\"");
        assert_eq!(
            windows_command_line(&["plain", "sp ace"]).unwrap(),
            "\"plain\" \"sp ace\""
        );
        assert_eq!(windows_command_line(&["a\"b"]).unwrap(), "\"a\\\"b\"");
        assert_eq!(
            windows_command_line(&["C:\\last\\"]).unwrap(),
            "\"C:\\last\\\\\""
        );
        let tricky = format!("a{}\"b", "\\".repeat(2));
        assert_eq!(
            windows_command_line(&[&tricky]).unwrap(),
            format!("\"a{}\"b\"", "\\".repeat(5))
        );
        assert_eq!(
            windows_command_line(&["Tiếng Việt"]).unwrap(),
            "\"Tiếng Việt\""
        );
        assert!(windows_command_line(&["bad\0argument"]).is_err());
    }

    #[test]
    fn malformed_drives_and_file_boundary_names_are_rejected() {
        for name in [
            "../outside",
            "CON",
            "con.txt",
            "lpt1",
            "com9.ini",
            "bad\rname",
            "trailing.",
            " COM1",
            "COM¹",
            "lpt²",
            "COM³",
        ] {
            assert!(
                environment_name(name).is_err(),
                "unexpectedly accepted {name:?}"
            );
        }
        assert_eq!(environment_name("Prod Europe").unwrap(), "prod europe");
        assert_eq!(drives("c:, D:;c:").unwrap().as_deref(), Some("C:;D:"));
        for value in [
            "C:;",
            "C:\\folder",
            "Ł:",
            "C:\r\nredirectprinters:i:1",
            "*,C:",
        ] {
            assert!(drives(value).is_err());
        }
    }
}
