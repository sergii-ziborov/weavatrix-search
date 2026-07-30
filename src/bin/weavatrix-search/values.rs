use super::{CliColor, ContentDiscoveryMode, EncodingMode, OsString, PathBuf};

pub(super) fn next_string(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))?
        .into_string()
        .map_err(|_| format!("{option} value must be valid UTF-8"))
}

pub(super) fn next_path(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<PathBuf, String> {
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{option} requires a path"))
}

pub(super) fn next_number<T>(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<T, String>
where
    T: std::str::FromStr,
{
    let value = next_string(arguments, option)?;
    parse_number(option, &value)
}

pub(super) fn parse_number<T>(option: &str, value: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    value
        .parse()
        .map_err(|_| format!("{option} requires a non-negative integer"))
}

pub(super) fn same_roots(indexed: &[PathBuf], requested: &[PathBuf]) -> bool {
    indexed.len() == requested.len()
        && indexed.iter().zip(requested).all(|(left, right)| {
            let left = left.canonicalize().unwrap_or_else(|_| left.clone());
            let right = right.canonicalize().unwrap_or_else(|_| right.clone());
            left == right
        })
}

pub(super) fn parse_encoding(value: &str) -> Result<EncodingMode, String> {
    match value.to_ascii_lowercase().as_str() {
        "auto" => Ok(EncodingMode::Auto),
        "utf-8" | "utf8" => Ok(EncodingMode::Utf8),
        "utf-16le" | "utf16le" => Ok(EncodingMode::Utf16Le),
        "utf-16be" | "utf16be" => Ok(EncodingMode::Utf16Be),
        _ if encoding_rs::Encoding::for_label(value.as_bytes()).is_some() => {
            Ok(EncodingMode::Label(value.to_owned()))
        }
        _ => Err(format!("unknown encoding label {value}")),
    }
}

pub(super) fn parse_color(value: &str) -> Result<CliColor, String> {
    match value {
        "auto" => Ok(CliColor::Auto),
        "always" => Ok(CliColor::Always),
        "never" => Ok(CliColor::Never),
        _ => Err(format!(
            "invalid color mode {value:?}; expected auto, always, or never"
        )),
    }
}

pub(super) fn parse_discovery(value: &str) -> Result<Option<ContentDiscoveryMode>, String> {
    match value {
        "adaptive" => Ok(None),
        "streaming" => Ok(Some(ContentDiscoveryMode::Streaming)),
        "buffered" => Ok(Some(ContentDiscoveryMode::BufferedParallel)),
        _ => Err(format!(
            "invalid discovery mode {value:?}; expected adaptive, streaming, or buffered"
        )),
    }
}
