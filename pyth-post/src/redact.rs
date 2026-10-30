//! Redaction of URLs and secrets from error text: Hermes and RPC providers put API keys in the
//! URL path or query, and HTTP clients quote the request URL.

/// Shortest URL path segment or query value treated as a possible key.
const MIN_SECRET_LEN: usize = 8;

/// The parts of `url` that can carry a key: path segments and query values of at least
/// [`MIN_SECRET_LEN`] characters (`/?api-key=KEY`, `/TOKEN/`), longest first.
fn secrets_of(url: &str) -> Vec<&str> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    // Drop the host: everything up to the first `/` or `?`.
    let tail = after_scheme
        .find(['/', '?'])
        .and_then(|i| after_scheme.split_at_checked(i))
        .map_or("", |(_, tail)| tail);
    let mut parts: Vec<&str> = tail
        .split(['/', '?', '&', '=', '#'])
        .filter(|p| p.len() >= MIN_SECRET_LEN)
        .collect();
    parts.sort_by_key(|p| std::cmp::Reverse(p.len()));
    parts
}

/// `text` with `url`, the API key, every path segment or query value of `url` of 8 or more
/// characters, and every other `http://` or `https://` URL replaced by placeholders. The HTTP
/// client may quote a normalized form of the URL, so exact replacement of it is not enough.
/// `hidden` replaces each URL; `key` may be empty.
#[must_use]
pub fn scrub(text: &str, url: &str, key: &str, hidden: &str) -> String {
    let mut text = if url.is_empty() {
        text.to_owned()
    } else {
        text.replace(url, hidden)
    };
    if !key.is_empty() {
        text = text.replace(key, "<redacted>");
    }
    for secret in secrets_of(url) {
        text = text.replace(secret, "<redacted>");
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = ["http://", "https://"]
        .iter()
        .filter_map(|scheme| rest.find(scheme))
        .min()
    {
        let Some((before, tail)) = rest.split_at_checked(start) else {
            break;
        };
        out.push_str(before);
        out.push_str(hidden);
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '>'))
            .unwrap_or(tail.len());
        rest = tail.split_at_checked(end).map_or("", |(_, after)| after);
    }
    out.push_str(rest);
    out
}
