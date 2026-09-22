#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpsMode {
    Disabled,
    EnabledAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpsUpgradeResult {
    Upgraded { new_url: String },
    AlreadySecure { url: String },
    NonHttpScheme { url: String },
    ExemptLocal { url: String },
    InsecureAllowed { url: String },

    Invalid { url: String },

    InsecureBlocked { url: String },
}

impl HttpsUpgradeResult {
    pub fn is_blocked(&self) -> bool {
        matches!(
            self,
            HttpsUpgradeResult::Invalid { .. } | HttpsUpgradeResult::InsecureBlocked { .. }
        )
    }
}

pub struct HttpsUpgradeEngine {
    pub mode: HttpsMode,
    pub exemptions: Vec<String>,
}

impl HttpsUpgradeEngine {
    pub fn new(mode: HttpsMode) -> Self {
        Self {
            mode,
            exemptions: vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "[::1]".to_string(),
            ],
        }
    }

    pub fn evaluate_url(&self, url: &str) -> HttpsUpgradeResult {
        let parsed = match url::Url::parse(url) {
            Ok(parsed) => parsed,
            Err(_) => {
                return HttpsUpgradeResult::Invalid {
                    url: url.to_string(),
                };
            }
        };
        if parsed.scheme() == "https" {
            return HttpsUpgradeResult::AlreadySecure {
                url: url.to_string(),
            };
        }
        if parsed.scheme() != "http" {
            return HttpsUpgradeResult::NonHttpScheme {
                url: url.to_string(),
            };
        }

        let domain = parsed.host_str().unwrap_or("");
        if is_exempt_local(domain, &self.exemptions) {
            return HttpsUpgradeResult::ExemptLocal {
                url: url.to_string(),
            };
        }

        match self.mode {
            HttpsMode::Disabled => HttpsUpgradeResult::InsecureAllowed {
                url: url.to_string(),
            },
            HttpsMode::EnabledAll => {
                let mut upgraded = parsed;
                let _ = upgraded.set_scheme("https");

                if upgraded.port() == Some(80) {
                    let _ = upgraded.set_port(None);
                }
                HttpsUpgradeResult::Upgraded {
                    new_url: upgraded.into(),
                }
            }
        }
    }
}

fn normalize_exempt_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches([']', '.'])
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn is_exempt_local(host: &str, exemptions: &[String]) -> bool {
    let normalized = normalize_exempt_host(host);
    if normalized.is_empty() {
        return false;
    }

    if normalized
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
    {
        return true;
    }
    if normalized == "localhost" {
        return true;
    }
    exemptions
        .iter()
        .map(|ex| normalize_exempt_host(ex))
        .any(|ex| {
            if ex.is_empty() {
                return false;
            }
            if ex
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
            {
                return normalized == ex;
            }
            normalized == ex
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_upgrade_evaluations() {
        let engine = HttpsUpgradeEngine::new(HttpsMode::EnabledAll);

        assert_eq!(
            engine.evaluate_url("http://example.com/login"),
            HttpsUpgradeResult::Upgraded {
                new_url: "https://example.com/login".to_string()
            }
        );

        assert_eq!(
            engine.evaluate_url("https://secure.com"),
            HttpsUpgradeResult::AlreadySecure {
                url: "https://secure.com".to_string()
            }
        );

        assert_eq!(
            engine.evaluate_url("http://localhost:8080"),
            HttpsUpgradeResult::ExemptLocal {
                url: "http://localhost:8080".to_string()
            }
        );
    }
}
