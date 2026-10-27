//! Windows autostart: HKCU `Run` key management.
//!
//! Only the current user is touched — no admin rights are needed, and the
//! setting survives only as long as the executable path stays the same.

use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};
use winreg::RegKey;

const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "MusicPresence";

/// Adds or removes the `MusicPresence` entry in the per-user Run key.
pub fn set_enabled(enable: bool) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(RUN_SUBKEY, KEY_SET_VALUE)
        .map_err(|e| format!("открытие ключа автозапуска: {e}"))?;
    if enable {
        let exe = std::env::current_exe().map_err(|e| format!("путь до exe: {e}"))?;
        key.set_value(VALUE_NAME, &format!("\"{}\"", exe.display()))
            .map_err(|e| format!("запись значения автозапуска: {e}"))?;
    } else {
        // Removing a value that is already gone is not an error.
        let _ = key.delete_value(VALUE_NAME);
    }
    Ok(())
}

/// Reads the current autostart state from the registry.
pub fn is_enabled() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey_with_flags(RUN_SUBKEY, KEY_QUERY_VALUE) else {
        return false;
    };
    key.get_value::<String, _>(VALUE_NAME)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    // No unit tests here: touching the real HKCU Run key from tests would
    // modify the developer machine. `set_enabled`/`is_enabled` are verified
    // manually by toggling the checkbox in the GUI.
}
