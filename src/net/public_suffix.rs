use std::collections::HashSet;
use std::sync::OnceLock;

const LIST: &str = include_str!("public_suffix_list.dat");

struct Rules {
    exact: HashSet<&'static str>,

    wildcard_bases: HashSet<&'static str>,

    exceptions: HashSet<&'static str>,
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut parsed = Rules {
            exact: HashSet::new(),
            wildcard_bases: HashSet::new(),
            exceptions: HashSet::new(),
        };
        for line in LIST.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }

            let rule = line.split_whitespace().next().unwrap_or_default();
            if rule.is_empty() {
                continue;
            }
            if let Some(exception) = rule.strip_prefix('!') {
                parsed.exceptions.insert(exception);
            } else if let Some(base) = rule.strip_prefix("*.") {
                parsed.wildcard_bases.insert(base);
            } else {
                parsed.exact.insert(rule);
            }
        }
        parsed
    })
}

fn public_suffix_label_count(labels: &[&str]) -> usize {
    let n = labels.len();

    for k in (1..=n).rev() {
        let candidate = labels[n - k..].join(".");
        if rules().exceptions.contains(candidate.as_str()) {
            return k - 1;
        }
    }

    for k in (1..=n).rev() {
        let candidate = labels[n - k..].join(".");
        if rules().exact.contains(candidate.as_str()) {
            return k;
        }

        if k >= 2
            && rules()
                .wildcard_bases
                .contains(labels[n - k + 1..].join(".").as_str())
        {
            return k;
        }
    }

    1
}

fn normalized_labels(domain: &str) -> Option<Vec<&str>> {
    let domain = domain.trim().trim_end_matches('.');
    if domain.is_empty() {
        return None;
    }
    let mut labels = Vec::new();
    for part in domain.split('.') {
        let label = part.trim();

        if label.is_empty() {
            return None;
        }
        labels.push(label);
    }
    if labels.is_empty() {
        return None;
    }
    Some(labels)
}

pub fn is_public_suffix(domain: &str) -> bool {
    let Some(labels) = normalized_labels(domain) else {
        return false;
    };
    let lowered: Vec<String> = labels.iter().map(|l| l.to_ascii_lowercase()).collect();
    let refs: Vec<&str> = lowered.iter().map(String::as_str).collect();
    public_suffix_label_count(&refs) == refs.len()
}

pub fn registrable_domain(host: &str) -> Option<String> {
    let labels = normalized_labels(host)?;
    let lowered: Vec<String> = labels.iter().map(|l| l.to_ascii_lowercase()).collect();
    let refs: Vec<&str> = lowered.iter().map(String::as_str).collect();
    let suffix_labels = public_suffix_label_count(&refs);
    if suffix_labels >= refs.len() {
        return None;
    }
    Some(refs[refs.len() - suffix_labels - 1..].join("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_suffixes_are_recognized() {
        assert!(is_public_suffix("com"));
        assert!(is_public_suffix("co.uk"));
        assert!(is_public_suffix("github.io"));
        assert!(is_public_suffix("pages.dev"));
        assert!(!is_public_suffix("example.com"));
        assert!(!is_public_suffix("user.github.io"));
    }

    #[test]
    fn wildcard_and_exception_rules_behave_per_spec() {
        assert!(is_public_suffix("foo.ck"));

        assert!(!is_public_suffix("www.ck"));
        assert_eq!(registrable_domain("www.ck").as_deref(), Some("www.ck"));
        assert_eq!(
            registrable_domain("site.foo.ck").as_deref(),
            Some("site.foo.ck")
        );
    }

    #[test]
    fn registrable_domains_follow_etld_plus_one() {
        assert_eq!(
            registrable_domain("a.b.example.co.uk").as_deref(),
            Some("example.co.uk")
        );
        assert_eq!(
            registrable_domain("user1.github.io").as_deref(),
            Some("user1.github.io")
        );
        assert_ne!(
            registrable_domain("user1.github.io"),
            registrable_domain("user2.github.io")
        );

        assert_eq!(registrable_domain("co.uk"), None);
        assert_eq!(registrable_domain("github.io"), None);
    }
}
