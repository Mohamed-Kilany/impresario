//! Secret redaction (spec 5.6): any observed text equal to a resolved
//! `${env.*}` value is replaced by `***` in reports, logs and the console.
//!
//! Source text (`'${env.TEST_PASSWORD}'`) is never redacted because
//! reporters only ever see the *unresolved* source; redaction applies to
//! wire messages, observed text, error details and log lines.

/// Replaces every recorded secret value with `***`.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    #[must_use]
    pub fn new(secrets: Vec<String>) -> Self {
        // Longest first so overlapping values cannot leave fragments behind;
        // empty values are dropped (they would match everywhere).
        let mut secrets: Vec<String> = secrets.into_iter().filter(|s| !s.is_empty()).collect();
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Self { secrets }
    }

    /// True when nothing needs redacting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// Redact every recorded secret value in `text`.
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        if self.secrets.is_empty() || text.is_empty() {
            return text.to_string();
        }
        let mut out = text.to_string();
        for secret in &self.secrets {
            if secret.is_empty() {
                continue;
            }
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "***");
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_recorded_secrets() {
        let redactor = Redactor::new(vec!["hunter2".to_string()]);
        assert_eq!(redactor.redact("pw=hunter2!"), "pw=***!");
        assert_eq!(redactor.redact("nothing"), "nothing");
        assert!(!redactor.is_empty());
    }

    #[test]
    fn empty_secrets_are_ignored() {
        let redactor = Redactor::new(vec![String::new(), "x".to_string()]);
        assert_eq!(redactor.redact("axb"), "a***b");
    }

    #[test]
    fn longest_secret_wins_over_prefix() {
        let redactor = Redactor::new(vec!["abc".to_string(), "abcdef".to_string()]);
        assert_eq!(redactor.redact("abcdef"), "***");
    }

    #[test]
    fn default_is_a_no_op() {
        let redactor = Redactor::default();
        assert!(redactor.is_empty());
        assert_eq!(redactor.redact("secret"), "secret");
    }
}
