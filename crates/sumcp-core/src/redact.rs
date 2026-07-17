//! Secret redaction for outbound excerpts (ADR A9(4)).
//!
//! Transcript content is attacker-influenceable and full of the user's real
//! shell commands and error dumps — exported tokens, connection strings, the
//! occasional pasted private key. Anything `evidence()` (and later the HTML
//! report) excerpts passes through here first.
//!
//! This is a deliberately conservative, dependency-free heuristic pass, not a
//! secret scanner: it redacts (1) PEM blocks, (2) words carrying well-known
//! secret prefixes, (3) values assigned to sensitive-looking keys, and
//! (4) bearer tokens. False positives (redacting something harmless) are
//! acceptable; false negatives are the failure mode that matters.

/// Well-known secret prefixes (case-sensitive, as issued). A word starting
/// with one of these and long enough to be a real credential is redacted.
const SECRET_PREFIXES: &[&str] = &[
    "sk-",      // OpenAI/Anthropic-style API keys
    "sk_live_", // Stripe live
    "ghp_",     // GitHub personal access token
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",        // GitHub app/oauth tokens
    "github_pat_", // GitHub fine-grained PAT
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-",  // Slack
    "glpat-", // GitLab
    "AKIA",   // AWS access key id
    "eyJ",    // JWT (base64 of `{"`)
];
/// Minimum length before a prefix match counts as a credential — keeps
/// short innocents like `sk-8` in prose from tripping the scanner.
const MIN_TOKEN_LEN: usize = 16;
/// Substrings that mark an assignment's key as sensitive (lowercased match).
const SENSITIVE_KEYS: &[&str] = &["key", "token", "secret", "passw", "credential", "auth"];

/// Redact secrets from `input`. Whitespace between words collapses to single
/// spaces within a line; line structure is preserved.
pub fn redact(input: &str) -> String {
    let mut out = redact_pem_blocks(input);
    out = out.lines().map(redact_line).collect::<Vec<_>>().join("\n");
    // `.lines()` drops a trailing newline; put it back to stay faithful.
    if input.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Replace every `-----BEGIN …-----` … `-----END …-----` block wholesale.
fn redact_pem_blocks(input: &str) -> String {
    let mut s = input.to_string();
    while let Some(start) = s.find("-----BEGIN") {
        // Find the end marker's closing dashes; a truncated block (no END —
        // exactly what a 600-char excerpt cap produces) redacts to the end.
        let end = s[start..]
            .find("-----END")
            .and_then(|e| {
                let after = start + e + "-----END".len();
                s[after..].find("-----").map(|d| after + d + "-----".len())
            })
            .unwrap_or(s.len());
        s.replace_range(start..end, "[REDACTED:pem]");
    }
    s
}

fn redact_line(line: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut after_bearer = false;
    for word in line.split_whitespace() {
        let w = if after_bearer || is_prefixed_token(word) {
            "[REDACTED]".to_string()
        } else if let Some(masked) = mask_sensitive_assignment(word) {
            masked
        } else {
            word.to_string()
        };
        // `Bearer <token>` / `Authorization: Bearer xyz` — redact the *next* word.
        after_bearer = word.eq_ignore_ascii_case("bearer");
        words.push(w);
    }
    words.join(" ")
}

/// `ghp_…`, `sk-…`, `AKIA…`, JWTs — the word *is* the secret.
fn is_prefixed_token(word: &str) -> bool {
    word.len() >= MIN_TOKEN_LEN && SECRET_PREFIXES.iter().any(|p| word.starts_with(p))
}

/// `API_KEY=hunter2` / `--token=abc` / `password: x` → keep the key, mask the
/// value. Returns `None` when the word isn't a sensitive assignment.
fn mask_sensitive_assignment(word: &str) -> Option<String> {
    let sep = word.find(['=', ':'])?;
    let (key, value) = word.split_at(sep);
    if value.len() <= 1 {
        return None; // bare `key=` or a trailing colon — nothing to hide
    }
    let key_lc = key.to_lowercase();
    SENSITIVE_KEYS
        .iter()
        .any(|k| key_lc.contains(k))
        .then(|| format!("{key}{}[REDACTED]", &value[..1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_api_key_value_is_masked_key_kept() {
        let r = redact("export API_KEY=abc123def456 && run");
        assert_eq!(r, "export API_KEY=[REDACTED] && run");
    }

    #[test]
    fn known_prefix_tokens_are_redacted_whole() {
        let r = redact("curl -H ghp_16charslong4sure done");
        assert_eq!(r, "curl -H [REDACTED] done");
        let jwt = format!("eyJ{}", "a".repeat(40));
        assert_eq!(redact(&jwt), "[REDACTED]");
    }

    #[test]
    fn short_prefix_lookalikes_survive() {
        assert_eq!(
            redact("sk-8 is a heat sink model"),
            "sk-8 is a heat sink model"
        );
    }

    #[test]
    fn bearer_token_is_redacted() {
        let r = redact("Authorization: Bearer abc.def.ghi");
        assert_eq!(r, "Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn pem_block_is_redacted_even_when_truncated() {
        let full =
            "before -----BEGIN RSA PRIVATE KEY-----\nMIIE...\n-----END RSA PRIVATE KEY----- after";
        assert_eq!(redact(full), "before [REDACTED:pem] after");
        // excerpt cap can cut the END marker off — still redacts to the end
        let cut = "x -----BEGIN PRIVATE KEY-----\nMIIE...";
        assert_eq!(redact(cut), "x [REDACTED:pem]");
    }

    #[test]
    fn password_colon_form_is_masked() {
        assert_eq!(
            redact("db password:hunter2 ok"),
            "db password:[REDACTED] ok"
        );
    }

    #[test]
    fn clean_text_passes_through() {
        let s = "cargo test -p sumcp-mcp\nline two";
        assert_eq!(redact(s), s);
    }
}
