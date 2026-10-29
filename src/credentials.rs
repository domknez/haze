//! Stored passwords, kept in the OS keychain: one secret per source (an Xtream Codes
//! password, or a playlist link that carries a login), by the source's id.

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

const SERVICE: &str = "haze";
/// One keychain item holds every secret, so macOS asks for access once, not once
/// per source.
const ACCOUNT: &str = "logins";
/// The platform's secure store, installed as keyring-core's default once. Tests get
/// an in-memory one: they never touch (or prompt for) the real keychain.
static STORE: LazyLock<Result<(), keyring_core::Error>> = LazyLock::new(|| {
    #[cfg(test)]
    let store = keyring_core::mock::Store::new()?;
    #[cfg(all(not(test), target_os = "macos"))]
    let store = apple_native_keyring_store::keychain::Store::new()?;
    #[cfg(all(not(test), windows))]
    let store = windows_native_keyring_store::Store::new()?;
    #[cfg(all(not(test), unix, not(target_os = "macos")))]
    let store = dbus_secret_service_keyring_store::Store::new()?;
    keyring_core::set_default_store(store);
    Ok(())
});

/// The stored logins, read from the keychain on first use and kept here after.
static LOGINS: Mutex<Option<Logins>> = Mutex::new(None);

/// Each source's secret by its login key, kept as one JSON object
/// (`{"<source id>": "<secret>"}`).
#[derive(Default, Clone, PartialEq, Debug)]
struct Logins(BTreeMap<String, String>);

impl Logins {
    fn to_json(&self) -> String {
        serde_json::json!(self.0).to_string()
    }

    /// Every login in the item; fields that aren't text are skipped.
    fn from_json(text: &str) -> Logins {
        let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
        let logins = v.as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())));
        Logins(logins.collect())
    }
}

fn entry_for(account: &str) -> Option<keyring_core::Entry> {
    if let Err(e) = &*STORE {
        log_warn!("no keychain available: {e}");
        return None;
    }
    keyring_core::Entry::new(SERVICE, account).ok()
}

/// Run `f` on the logins, reading them from the keychain the first time.
fn with_logins<R>(f: impl FnOnce(&mut Logins) -> R) -> R {
    let mut logins = LOGINS.lock().unwrap_or_else(|e| e.into_inner());
    f(logins.get_or_insert_with(read))
}

fn read() -> Logins {
    let Some(entry) = entry_for(ACCOUNT) else { return Logins::default() };
    match entry.get_password() {
        Ok(text) => Logins::from_json(&text),
        Err(keyring_core::Error::NoEntry) => Logins::default(),
        Err(e) => {
            log_warn!("could not read logins from the keychain: {e}");
            Logins::default()
        }
    }
}

fn write(logins: &Logins) {
    let Some(entry) = entry_for(ACCOUNT) else { return };
    let result = if *logins == Logins::default() { entry.delete_credential() } else { entry.set_password(&logins.to_json()) };
    match result {
        Ok(()) | Err(keyring_core::Error::NoEntry) => {}
        Err(e) => log_warn!("could not save logins to the keychain: {e}"),
    }
}

/// Change the logins and save them (only if they changed: every write to the
/// keychain may ask for access again).
fn update(f: impl FnOnce(&mut Logins)) {
    with_logins(|logins| {
        let before = logins.clone();
        f(logins);
        if *logins != before {
            write(logins);
        }
    });
}

/// The stored secret of source `id`, if any.
pub fn load(id: &str) -> Option<String> {
    with_logins(|l| l.0.get(id).cloned())
}

pub fn save(id: &str, secret: &str) {
    update(|l| {
        l.0.insert(id.to_string(), secret.to_string());
    });
}

pub fn clear(id: &str) {
    update(|l| {
        l.0.remove(id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logins_round_trip() {
        let both = Logins([("a1".to_string(), "pw".to_string()), ("b2".to_string(), "x\"y".to_string())].into());
        assert_eq!(Logins::from_json(&both.to_json()), both);
        assert_eq!(Logins::from_json("not json"), Logins::default());
        assert_eq!(Logins::from_json(r#"{"a":"1","b":null}"#).0.len(), 1);
    }

    #[test]
    fn saves_and_clears() {
        save("t1", "secret");
        assert_eq!(load("t1").as_deref(), Some("secret"));
        clear("t1");
        assert_eq!(load("t1"), None);
    }
}
