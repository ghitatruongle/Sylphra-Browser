use serde::{Deserialize, Serialize};

const MAX_VAULT_ENTRIES: usize = 256;
const MAX_PASSWORD_BYTES: usize = 512;
const MAX_USERNAME_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultEntry {
    pub id: String,
    pub origin: String,
    pub username: String,
    #[serde(rename = "protected")]
    pub protected_blob: Vec<u8>,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PasswordVault {
    pub entries: Vec<VaultEntry>,
    #[serde(default)]
    next_id: u64,
}

impl PasswordVault {
    pub fn add(&mut self, origin: &str, username: &str, password: &str) -> Result<String, String> {
        let normalized = normalize_origin(origin)?;
        let username = username.trim();
        if username.is_empty() || username.len() > MAX_USERNAME_BYTES {
            return Err("Vault username is invalid".to_string());
        }
        if password.is_empty() || password.len() > MAX_PASSWORD_BYTES {
            return Err("Vault password is invalid".to_string());
        }
        let protected_blob = protect_password(password)?;
        let existing = self
            .entries
            .iter_mut()
            .find(|entry| entry.origin == normalized && entry.username == username);
        if let Some(entry) = existing {
            entry.protected_blob = protected_blob;
            return Ok(entry.id.clone());
        }
        if self.entries.len() >= MAX_VAULT_ENTRIES {
            self.entries.remove(0);
        }
        let id = format!(
            "vault-{}-{}",
            chrono::Utc::now().timestamp_millis(),
            self.next_id
        );
        self.next_id = self.next_id.wrapping_add(1);
        let entry = VaultEntry {
            id: id.clone(),
            origin: normalized,
            username: username.to_string(),
            protected_blob,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.entries.push(entry);
        Ok(id)
    }

    pub fn find_for_origin(&self, url: &str) -> Vec<&VaultEntry> {
        let Some(host) = normalize_host(url) else {
            return Vec::new();
        };
        self.entries
            .iter()
            .filter(|entry| {
                let Some(stored) = normalize_host(&entry.origin) else {
                    return false;
                };
                host == stored || host.ends_with(&format!(".{stored}"))
            })
            .collect()
    }

    pub fn reveal(&self, id: &str) -> Option<(String, String)> {
        let entry = self.entries.iter().find(|entry| entry.id == id)?;
        let password = unprotect_password(&entry.protected_blob).ok()?;
        Some((entry.username.clone(), password))
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.id != id);
        self.entries.len() < before
    }

    pub fn load(path: &std::path::Path) -> PasswordVault {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        crate::fs_atomic::atomic_write_bytes(path, &bytes).map_err(|error| error.to_string())
    }
}

#[cfg(windows)]
fn protect_password(password: &str) -> Result<Vec<u8>, String> {
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input = password.as_bytes().to_vec();
    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input_blob,
            windows::core::PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|error| format!("DPAPI protect failed: {error}"))?;
    }
    zeroize(&mut input);
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        let _ = windows::Win32::Foundation::LocalFree(windows::Win32::Foundation::HLOCAL(
            output.pbData.cast(),
        ));
    }
    if bytes.is_empty() {
        return Err("DPAPI protect produced an empty blob".to_string());
    }
    Ok(bytes)
}

#[cfg(windows)]
fn unprotect_password(blob: &[u8]) -> Result<String, String> {
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input = blob.to_vec();
    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|error| format!("DPAPI unprotect failed: {error}"))?;
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        let _ = windows::Win32::Foundation::LocalFree(windows::Win32::Foundation::HLOCAL(
            output.pbData.cast(),
        ));
    }
    String::from_utf8(bytes).map_err(|_| "DPAPI output is not valid UTF-8".to_string())
}

#[cfg(not(windows))]
fn protect_password(_password: &str) -> Result<Vec<u8>, String> {
    Err("DPAPI is unavailable on this platform".to_string())
}

#[cfg(not(windows))]
fn unprotect_password(_blob: &[u8]) -> Result<String, String> {
    Err("DPAPI is unavailable on this platform".to_string())
}

fn zeroize(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

fn normalize_origin(origin: &str) -> Result<String, String> {
    let parsed =
        url::Url::parse(origin.trim()).map_err(|_| "Vault origin is invalid".to_string())?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err("Vault entries require an HTTPS origin".to_string());
    }
    Ok(parsed.origin().ascii_serialization())
}

fn normalize_host(input: &str) -> Option<String> {
    let parsed = url::Url::parse(input.trim()).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn vault_roundtrip_reveals_protected_password() {
        let mut vault = PasswordVault::default();
        let id = vault
            .add("https://example.com/login", "alice", "hunter2secret")
            .unwrap();
        let (username, password) = vault.reveal(&id).unwrap();
        assert_eq!(username, "alice");
        assert_eq!(password, "hunter2secret");
        assert!(
            !vault.entries[0]
                .protected_blob
                .windows(13)
                .any(|slice| slice == b"hunter2secret"),
            "plaintext must never appear in the protected blob"
        );
    }

    #[cfg(windows)]
    #[test]
    fn tampered_blob_fails_to_reveal() {
        let mut vault = PasswordVault::default();
        let id = vault
            .add("https://example.com", "bob", "correct-horse")
            .unwrap();
        let last = vault.entries[0].protected_blob.len() - 1;
        vault.entries[0].protected_blob[last] ^= 0xFF;
        assert!(vault.reveal(&id).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn vault_file_has_no_plaintext_password() {
        let temp =
            std::env::temp_dir().join(format!("sylphra-vault-test-{}.json", std::process::id()));
        let mut vault = PasswordVault::default();
        vault
            .add("https://example.com", "carol", "super-secret-99")
            .unwrap();
        vault.save(&temp).unwrap();
        let bytes = std::fs::read(&temp).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("super-secret-99"));
        let loaded = PasswordVault::load(&temp);
        let id = loaded.entries[0].id.clone();
        assert_eq!(loaded.reveal(&id).unwrap().1, "super-secret-99");
        let _ = std::fs::remove_file(&temp);
    }

    #[test]
    fn vault_requires_https_origins() {
        let mut vault = PasswordVault::default();
        assert!(vault.add("http://example.com", "u", "p").is_err());
        assert!(vault.add("ftp://example.com", "u", "p").is_err());
    }

    #[test]
    fn vault_upserts_same_origin_and_username() {
        let mut vault = PasswordVault::default();
        let first = vault.add("https://example.com", "dana", "one").unwrap();
        let second = vault.add("https://example.com", "dana", "two").unwrap();
        assert_eq!(first, second);
        assert_eq!(vault.entries.len(), 1);
        assert_eq!(vault.reveal(&second).unwrap().1, "two");
    }

    #[test]
    fn vault_matches_subdomains_without_substring_hits() {
        let mut vault = PasswordVault::default();
        vault.add("https://example.com", "u", "p").unwrap();
        vault.add("https://api.example.com", "v", "q").unwrap();
        assert_eq!(vault.find_for_origin("https://example.com").len(), 1);
        assert_eq!(vault.find_for_origin("https://www.example.com").len(), 1);
        assert_eq!(vault.find_for_origin("https://api.example.com").len(), 2);
        assert_eq!(vault.find_for_origin("https://badexample.com").len(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn vault_caps_entries_at_budget() {
        let mut vault = PasswordVault::default();
        for index in 0..(MAX_VAULT_ENTRIES + 10) {
            let _ = vault.add(
                &format!("https://host{index}.example.com"),
                "u",
                &format!("p{index}"),
            );
        }
        assert!(vault.entries.len() <= MAX_VAULT_ENTRIES);
    }
}
