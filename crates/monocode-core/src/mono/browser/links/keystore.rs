//! Storage of the pairing key K on the app side (spec 5.4): the OS credential store where one
//! exists, otherwise the encrypted `mono_secrets` table. The manager only sees [`LinkKeyStore`],
//! so tests inject [`MemoryKeyStore`] and never touch a real keychain.
use super::super::link::auth::PairingKey;
use super::super::MonoBrowserStore;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Keyed by the extension's `instance_id`. Errors are user-safe text, never the key.
pub(crate) trait LinkKeyStore: Send + Sync {
    fn get(&self, instance_id: &str) -> Result<Option<PairingKey>, String>;
    fn set(&self, instance_id: &str, key: &PairingKey) -> Result<(), String>;
    fn delete(&self, instance_id: &str) -> Result<(), String>;
}

/// Service name of the credential-store items. Native Dev uses its own so it never reads or
/// deletes the installed app's keys.
pub(crate) fn keyring_service(dev: bool) -> &'static str {
    if dev {
        "app.zptah.browser-link.dev"
    } else {
        "app.zptah.browser-link"
    }
}

/// The OS credential store through the `keyring` crate.
pub(crate) struct KeyringStore {
    service: &'static str,
}

impl KeyringStore {
    pub(crate) fn new(dev: bool) -> Self {
        Self {
            service: keyring_service(dev),
        }
    }

    fn entry(&self, instance_id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(self.service, instance_id)
            .map_err(|_| "The credential store is unavailable.".to_string())
    }
}

impl LinkKeyStore for KeyringStore {
    fn get(&self, instance_id: &str) -> Result<Option<PairingKey>, String> {
        match self.entry(instance_id)?.get_password() {
            Ok(text) => PairingKey::from_b64(&text)
                .map(Some)
                .map_err(|_| "The saved browser key is damaged.".to_string()),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("The credential store could not be read.".to_string()),
        }
    }

    fn set(&self, instance_id: &str, key: &PairingKey) -> Result<(), String> {
        self.entry(instance_id)?
            .set_password(&key.to_b64())
            .map_err(|_| "The credential store could not be written.".to_string())
    }

    fn delete(&self, instance_id: &str) -> Result<(), String> {
        match self.entry(instance_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("The credential store could not be written.".to_string()),
        }
    }
}

/// The `mono_secrets` fallback. Rows use a fixed pseudo Mono id and the instance id as the name,
/// so they are sealed like every other secret and bound to that pair by the AAD.
pub(crate) struct MonoSecretsKeyStore {
    store: MonoBrowserStore,
}

/// Not a real Mono: groups the link keys in `mono_secrets`.
const SECRETS_OWNER: &str = "browser-links";

impl MonoSecretsKeyStore {
    pub(crate) fn new(store: MonoBrowserStore) -> Self {
        Self { store }
    }

    fn name(instance_id: &str) -> String {
        format!("link-key-{instance_id}")
    }
}

impl LinkKeyStore for MonoSecretsKeyStore {
    fn get(&self, instance_id: &str) -> Result<Option<PairingKey>, String> {
        match self.store.secret(SECRETS_OWNER, &Self::name(instance_id))? {
            Some(secret) => PairingKey::from_b64(secret.as_str())
                .map(Some)
                .map_err(|_| "The saved browser key is damaged.".to_string()),
            None => Ok(None),
        }
    }

    fn set(&self, instance_id: &str, key: &PairingKey) -> Result<(), String> {
        self.store
            .set_secret(SECRETS_OWNER, &Self::name(instance_id), &key.to_b64())
    }

    fn delete(&self, instance_id: &str) -> Result<(), String> {
        self.store
            .delete_secret(SECRETS_OWNER, &Self::name(instance_id))
    }
}

/// Prefers `primary` (the credential store). A failing primary falls back to `fallback` for
/// writes, and reads look in both, so a machine without a working credential store still pairs.
/// A delete removes the key from both.
pub(crate) struct FallbackKeyStore {
    primary: Arc<dyn LinkKeyStore>,
    fallback: Arc<dyn LinkKeyStore>,
}

impl FallbackKeyStore {
    pub(crate) fn new(primary: Arc<dyn LinkKeyStore>, fallback: Arc<dyn LinkKeyStore>) -> Self {
        Self { primary, fallback }
    }
}

impl LinkKeyStore for FallbackKeyStore {
    fn get(&self, instance_id: &str) -> Result<Option<PairingKey>, String> {
        match self.primary.get(instance_id) {
            Ok(Some(key)) => Ok(Some(key)),
            Ok(None) | Err(_) => self.fallback.get(instance_id),
        }
    }

    fn set(&self, instance_id: &str, key: &PairingKey) -> Result<(), String> {
        match self.primary.set(instance_id, key) {
            Ok(()) => {
                // A stale copy in the fallback must not outlive a rotated key.
                let _ = self.fallback.delete(instance_id);
                Ok(())
            }
            Err(_) => self.fallback.set(instance_id, key),
        }
    }

    fn delete(&self, instance_id: &str) -> Result<(), String> {
        let first = self.primary.delete(instance_id);
        let second = self.fallback.delete(instance_id);
        first.and(second)
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub(crate) struct MemoryKeyStore {
    keys: Mutex<HashMap<String, PairingKey>>,
    fail_writes: Mutex<bool>,
}

impl MemoryKeyStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Makes `set` fail, to test the fallback and the pairing error path.
    pub(crate) fn fail_writes(&self, fail: bool) {
        *self.fail_writes.lock().unwrap() = fail;
    }

    pub(crate) fn len(&self) -> usize {
        self.keys.lock().unwrap().len()
    }
}

impl LinkKeyStore for MemoryKeyStore {
    fn get(&self, instance_id: &str) -> Result<Option<PairingKey>, String> {
        Ok(self.keys.lock().unwrap().get(instance_id).cloned())
    }

    fn set(&self, instance_id: &str, key: &PairingKey) -> Result<(), String> {
        if *self.fail_writes.lock().unwrap() {
            return Err("write refused".to_string());
        }
        self.keys
            .lock()
            .unwrap()
            .insert(instance_id.to_string(), key.clone());
        Ok(())
    }

    fn delete(&self, instance_id: &str) -> Result<(), String> {
        self.keys.lock().unwrap().remove(instance_id);
        Ok(())
    }
}
