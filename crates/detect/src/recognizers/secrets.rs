use crate::util::{lazy_regex, trim_end};
use crate::{DetectError, Detection, EntityType, Recognizer};
use regex::Regex;
use std::sync::LazyLock;

struct Rule {
    id: &'static str,
    entity: EntityType,
    score: f32,
    /// Capture group holding the secret; 0 = whole match.
    group: usize,
    re: Regex,
}

fn rule(id: &'static str, entity: EntityType, score: f32, group: usize, re: &str) -> Rule {
    Rule { id, entity, score, group, re: Regex::new(re).unwrap_or_else(|e| panic!("{id}: {e}")) }
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    use EntityType::*;
    vec![
        rule("secret.anthropic", ApiKey, 0.99, 0, r"\bsk-ant-[A-Za-z0-9_\-]{20,}"),
        rule("secret.openai", ApiKey, 0.95, 0, r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_\-]{20,}"),
        rule(
            "secret.aws_access_key",
            ApiKey,
            0.97,
            0,
            r"\b(?:AKIA|ASIA|ABIA|ACCA|AGPA|AIDA|AROA|ANPA|ANVA|APKA)[A-Z0-9]{16}\b",
        ),
        rule(
            "secret.aws_secret_key",
            ApiKey,
            0.97,
            1,
            r#"(?i)aws_?secret_?access_?key["']?\s*[:=]\s*["']?([A-Za-z0-9/+=]{40})"#,
        ),
        rule(
            "secret.github",
            ApiKey,
            0.99,
            0,
            r"\b(?:gh[pousr]_[A-Za-z0-9]{36,255}|github_pat_[A-Za-z0-9_]{22,255})\b",
        ),
        rule("secret.gitlab", ApiKey, 0.99, 0, r"\bglpat-[A-Za-z0-9_\-]{20,}"),
        rule("secret.stripe", ApiKey, 0.99, 0, r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}\b"),
        rule("secret.stripe_publishable", ApiKey, 0.7, 0, r"\bpk_(?:live|test)_[A-Za-z0-9]{16,}\b"),
        rule("secret.slack", ApiKey, 0.97, 0, r"\bxox[abposr]-[A-Za-z0-9\-]{10,}"),
        rule("secret.slack_webhook", ApiKey, 0.97, 0, r"https://hooks\.slack\.com/services/[A-Za-z0-9/_\-]+"),
        rule("secret.google_api", ApiKey, 0.97, 0, r"\bAIza[0-9A-Za-z_\-]{35}\b"),
        rule("secret.huggingface", ApiKey, 0.95, 0, r"\bhf_[A-Za-z0-9]{30,}\b"),
        rule("secret.sendgrid", ApiKey, 0.99, 0, r"\bSG\.[A-Za-z0-9_\-]{22}\.[A-Za-z0-9_\-]{43}\b"),
        rule("secret.npm", ApiKey, 0.97, 0, r"\bnpm_[A-Za-z0-9]{36}\b"),
        rule("secret.bearer", ApiKey, 0.9, 1, r"(?i)\bbearer\s+([A-Za-z0-9\-._~+/]{20,}=*)"),
        rule(
            "secret.private_key",
            PrivateKey,
            0.99,
            0,
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)",
        ),
        rule("secret.jwt", Jwt, 0.97, 0, r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{16,}"),
        rule(
            "secret.connection_string",
            ConnectionString,
            0.75,
            0,
            r#"(?i)\b(?:postgres(?:ql)?|mysql|mariadb|mongodb(?:\+srv)?|rediss?|amqps?|mssql|sqlserver|cockroachdb|clickhouse|jdbc:[a-z0-9]+)://[^\s'"<>]+"#,
        ),
        rule(
            "secret.ado_connection",
            ConnectionString,
            0.95,
            0,
            r"(?i)\b(?:server|data source|host)\s*=[^;\n]+;[^\n]*?(?:password|pwd)\s*=\s*[^;\n]+;?",
        ),
        rule("secret.url_credentials", Secret, 0.95, 0, r"(?i)\bhttps?://[^\s:/@]+:[^\s@/]+@[^\s]+"),
        rule("secret.azure_account_key", ApiKey, 0.97, 1, r"(?i)AccountKey=([A-Za-z0-9+/=]{60,})"),
    ]
});

// KEY=value lines (.env, shell exports, YAML-ish `key: value`).
lazy_regex!(
    ENV_LINE,
    r#"(?m)^[ \t]*(?:export[ \t]+)?([A-Za-z_][A-Za-z0-9_.\-]*)[ \t]*[=:][ \t]*(?:"([^"\n]*)"|'([^'\n]*)'|([^\s#]+))"#
);
lazy_regex!(
    SECRET_KEY_NAME,
    r"(?i)(?:key|secret|token|passw|pwd|pass\b|credential|auth|private|cert|salt|signing|session)"
);
// Inline mentions: "Passwort: hunter2", "das Kennwort lautet Abc123!".
lazy_regex!(
    INLINE_SECRET,
    r#"(?i)\b(?:passwor[dt]|kennwort|pwd|passwd|pin|api[_\-]?key|secret|token|zugangscode)\b\s*(?P<sep>[:=]|ist|lautet|is)\s*["']?(?P<v>[^\s"',;]{4,})"#
);

const PLACEHOLDER_VALUES: &[&str] =
    &["changeme", "xxx", "xxxx", "null", "none", "true", "false", "example", "your_key_here"];

/// API keys, private keys, JWTs, connection strings and `.env`-style secrets.
#[derive(Debug, Default, Clone, Copy)]
pub struct SecretsRecognizer;

impl Recognizer for SecretsRecognizer {
    fn id(&self) -> &str {
        "secrets"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::ApiKey, EntityType::PrivateKey, EntityType::Jwt, EntityType::ConnectionString, EntityType::Secret]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for r in RULES.iter() {
            for caps in r.re.captures_iter(text) {
                let Some(m) = caps.get(r.group) else { continue };
                if r.id == "secret.openai" && m.as_str().starts_with("sk-ant-") {
                    continue;
                }
                let mut end = m.end();
                let mut score = r.score;
                if r.entity == EntityType::ConnectionString && r.id == "secret.connection_string" {
                    end = trim_end(text, m.start(), end, &['.', ',', ';', ')', '"', '\'']);
                    // user:password@host makes this a credential.
                    let s = &text[m.start()..end];
                    if s.split("://").nth(1).is_some_and(|rest| {
                        rest.split('/').next().is_some_and(|auth| auth.contains(':') && auth.contains('@'))
                    }) {
                        score = 0.99;
                    }
                }
                out.push(Detection::new(m.start(), end, r.entity, score, r.id));
            }
        }
        for caps in ENV_LINE.captures_iter(text) {
            let key = &caps[1];
            if !SECRET_KEY_NAME.is_match(key) {
                continue;
            }
            let Some(v) = caps.get(2).or_else(|| caps.get(3)).or_else(|| caps.get(4)) else { continue };
            let value = v.as_str();
            if value.chars().count() < 4
                || value.starts_with("${")
                || value.starts_with('<')
                || value.eq_ignore_ascii_case("bearer")
                || value.eq_ignore_ascii_case("basic")
                || PLACEHOLDER_VALUES.contains(&value.to_ascii_lowercase().as_str())
            {
                continue;
            }
            out.push(Detection::new(v.start(), v.end(), EntityType::Secret, 0.9, "secret.env_assignment"));
        }
        for caps in INLINE_SECRET.captures_iter(text) {
            let v = caps.name("v").expect("group v");
            if out.iter().any(|d| d.start == v.start() && d.end == v.end()) {
                continue;
            }
            let value = v.as_str();
            let symbolic = matches!(&caps["sep"], ":" | "=");
            let looks_secret = value.chars().any(|c| c.is_ascii_digit() || c.is_ascii_punctuation());
            let score = match (symbolic, looks_secret) {
                (true, _) => 0.85,
                (false, true) => 0.8,
                (false, false) => 0.45,
            };
            out.push(Detection::new(v.start(), v.end(), EntityType::Secret, score, "secret.inline"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::test_util::found;

    fn ids(text: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        SecretsRecognizer.detect(text, &mut out).unwrap();
        out.into_iter().map(|d| (d.text(text).to_owned(), d.recognizer.into_owned())).collect()
    }

    #[test]
    fn provider_keys() {
        let ant = format!("sk-ant-api03-{}", "a".repeat(40));
        let oai = format!("sk-proj-{}", "B1".repeat(20));
        let gh = format!("ghp_{}", "x".repeat(36));
        let t = format!("ANT {ant} OAI {oai} GH {gh} AWS AKIAIOSFODNN7EXAMPLE STRIPE sk_live_{}", "9".repeat(24));
        let got = ids(&t);
        let names: Vec<_> = got.iter().map(|(_, id)| id.as_str()).collect();
        assert_eq!(
            names,
            ["secret.anthropic", "secret.openai", "secret.aws_access_key", "secret.github", "secret.stripe"]
        );
        assert_eq!(got[0].0, ant);
    }

    #[test]
    fn private_key_block() {
        let t = "foo\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH PRIVATE KEY-----\nbar";
        let got = found(&SecretsRecognizer, t, 0.9);
        assert_eq!(got.len(), 1);
        assert!(got[0].0.starts_with("-----BEGIN") && got[0].0.ends_with("KEY-----"));
        assert_eq!(got[0].1, EntityType::PrivateKey);
    }

    #[test]
    fn jwt_and_connection_strings() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let t = format!("token {jwt} und postgres://app:s3cret@db.internal:5432/prod.");
        let got = found(&SecretsRecognizer, &t, 0.9);
        assert_eq!(got[0], (jwt.to_owned(), EntityType::Jwt));
        assert_eq!(got[1], ("postgres://app:s3cret@db.internal:5432/prod".to_owned(), EntityType::ConnectionString));
    }

    #[test]
    fn env_lines_redact_value_only() {
        let t = "DATABASE_URL_HOST=localhost\nAPI_KEY=abcd1234efgh\nexport DB_PASSWORD=\"hunter2!\"\nDEBUG=true\nSECRET_KEY=${FROM_VAULT}";
        let got: Vec<_> = found(&SecretsRecognizer, t, 0.8).into_iter().map(|(s, _)| s).collect();
        assert_eq!(got, ["abcd1234efgh", "hunter2!"]);
    }

    #[test]
    fn inline_password() {
        let got = found(&SecretsRecognizer, "Das Passwort lautet Sommer2024! bitte nicht teilen", 0.8);
        assert_eq!(got, [("Sommer2024!".to_owned(), EntityType::Secret)]);
        assert!(found(&SecretsRecognizer, "Das Token ist abgelaufen", 0.5).is_empty());
        // The bearer rule owns `Authorization: Bearer …`; the env rule must not flag "Bearer".
        let t = "Authorization: Bearer abcdefghijklmnopqrstuvwxyz123456";
        assert_eq!(
            found(&SecretsRecognizer, t, 0.5),
            [("abcdefghijklmnopqrstuvwxyz123456".to_owned(), EntityType::ApiKey)]
        );
    }
}
