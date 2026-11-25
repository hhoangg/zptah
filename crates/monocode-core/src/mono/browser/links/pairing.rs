//! The pairing ticket the Connect card shows and the page the user is sent to (spec 12.3 steps 4
//! to 6). The nonce check itself is `link::auth::PairingWindow`.
use super::profiles::valid_profile_dir;

/// The browser profile a pairing is for. Recorded with the link so a copied profile cannot pose
/// as it (spec 5.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProfileChoice {
    /// `--profile-directory` value, for example "Default".
    pub dir: String,
    /// Display name for the card, for example "Work".
    pub name: String,
}

impl ProfileChoice {
    pub(crate) fn new(dir: impl Into<String>, name: impl Into<String>) -> Result<Self, String> {
        let dir = dir.into();
        if !valid_profile_dir(&dir) {
            return Err("That browser profile name is not valid.".to_string());
        }
        let name = name.into();
        let name = name.trim();
        let name: String = if name.is_empty() { &dir } else { name }
            .chars()
            .take(64)
            .collect();
        Ok(Self { dir, name })
    }
}

/// What `start_pairing` hands back: the nonce for the pair page and when the window closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PairingTicket {
    pub nonce: String,
    pub expires_ms: u64,
}

/// What the Connect card shows about pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PairingState {
    Idle,
    Waiting { expires_ms: u64 },
}

/// `chrome-extension://<id>/src/pair/pair.html#n=<nonce>`. The nonce is standard base64, so `+`,
/// `/` and `=` are percent-encoded: both `decodeURIComponent` and `URLSearchParams` give it back
/// unchanged.
pub(crate) fn pair_page_url(extension_id: &str, nonce: &str) -> String {
    let mut encoded = String::with_capacity(nonce.len() + 8);
    for b in nonce.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => encoded.push(b as char),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    format!("chrome-extension://{extension_id}/src/pair/pair.html#n={encoded}")
}

/// Arguments that open `url` in one profile of Chrome: `--profile-directory=<dir> <url>`. Returned
/// as data so callers (and tests) decide how to launch; the app never launches in tests.
pub(crate) fn open_in_profile_args(profile: &ProfileChoice, url: &str) -> Vec<String> {
    vec![
        format!("--profile-directory={}", profile.dir),
        url.to_string(),
    ]
}
