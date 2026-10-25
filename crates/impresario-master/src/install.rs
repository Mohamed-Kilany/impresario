//! First-run PATH setup for the `impresario` entry point.
//!
//! The user-facing contract is simple: running the binary once makes
//! `impresario` available in a new shell. We copy the running executable into
//! `$IMPRESARIO_HOME/bin` and add that directory to the user PATH. Nothing here
//! is allowed to make a command fail; callers report a warning and continue.
//!
//! `IMPRESARIO_HOME` is deliberately treated as an opt-out. Test harnesses and
//! portable installs set it, and they must not mutate the real user PATH.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use impresario_core::home::Home;

/// What the setup did, for the one-line notice printed by the CLI.
#[derive(Debug, Clone)]
pub struct Report {
    /// Directory containing the installed executable.
    pub bin_dir: PathBuf,
    /// Whether the user PATH (registry/profile) was changed.
    pub path_updated: bool,
    /// Whether a new executable was copied into the bin directory.
    pub copied: bool,
}

/// Ensure `impresario` is available on PATH.
///
/// Returns `Ok(None)` when setup is deliberately disabled. Errors are returned
/// for the caller to log; they never abort the actual command.
pub fn ensure_on_path(home: &Home) -> Result<Option<Report>, String> {
    if env::var_os("IMPRESARIO_NO_PATH_INSTALL").is_some()
        || env::var_os("IMPRESARIO_HOME").is_some()
    {
        return Ok(None);
    }

    let current = env::current_exe()
        .map_err(|error| format!("cannot locate the running executable: {error}"))?;
    let bin_dir = home.bin_dir();
    fs::create_dir_all(&bin_dir)
        .map_err(|error| format!("cannot create {}: {error}", bin_dir.display()))?;
    let destination = bin_dir.join(executable_name());
    let copied = if same_path(&current, &destination) {
        false
    } else {
        fs::copy(&current, &destination).map_err(|error| {
            format!(
                "cannot install {} to {}: {error}",
                current.display(),
                destination.display()
            )
        })?;
        set_executable(&destination)?;
        true
    };

    let path_updated = update_user_path(&bin_dir)?;
    update_process_path(&bin_dir);

    Ok(Some(Report {
        bin_dir,
        path_updated,
        copied,
    }))
}

fn executable_name() -> &'static str {
    if cfg!(windows) {
        "impresario.exe"
    } else {
        "impresario"
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn set_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
            .map_err(|error| format!("cannot mark {} executable: {error}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn path_list_contains(value: &OsStr, wanted: &Path) -> bool {
    env::split_paths(value).any(|entry| same_path(&entry, wanted))
}

#[cfg(any(windows, test))]
fn append_path_entry(existing: &str, entry: &str) -> String {
    if existing.trim().is_empty() {
        entry.to_string()
    } else if existing.ends_with(';') {
        // Keep an intentional trailing separator (it means "current directory"
        // on Windows) rather than silently changing the user's PATH semantics.
        format!("{existing}{entry}")
    } else {
        format!("{existing};{entry}")
    }
}

fn update_process_path(bin_dir: &Path) {
    let Some(current) = env::var_os("PATH") else {
        env::set_var("PATH", bin_dir.as_os_str());
        return;
    };
    if path_list_contains(&current, bin_dir) {
        return;
    }
    if let Ok(joined) = env::join_paths(env::split_paths(&current).chain([bin_dir.to_path_buf()])) {
        env::set_var("PATH", joined);
    }
}

#[cfg(windows)]
fn update_user_path(bin_dir: &Path) -> Result<bool, String> {
    use std::ffi::OsString;

    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let subkey = wide("Environment");
    let value_name = wide("Path");
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: the key and value names are NUL-terminated UTF-16 buffers.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if status != 0 {
        return Err(format!(
            "cannot open HKCU\\Environment (Windows error {status})"
        ));
    }

    let mut kind: REG_VALUE_TYPE = REG_SZ;
    let mut size = 0u32;
    // SAFETY: a null data pointer with size 0 asks the API for the byte size.
    let query = unsafe {
        RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    let existing = if query == 0 {
        let mut bytes = vec![0u8; size as usize];
        // SAFETY: `bytes` is writable for exactly `size` bytes.
        let status = unsafe {
            RegQueryValueExW(
                key,
                value_name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                bytes.as_mut_ptr(),
                &mut size,
            )
        };
        if status != 0 {
            unsafe { RegCloseKey(key) };
            return Err(format!(
                "cannot read HKCU\\Environment\\Path (Windows error {status})"
            ));
        }
        let utf16: Vec<u16> = bytes
            .chunks_exact(2)
            .filter_map(|pair| <[u8; 2]>::try_from(pair).ok())
            .map(u16::from_le_bytes)
            .take_while(|unit| *unit != 0)
            .collect();
        String::from_utf16_lossy(&utf16)
    } else if query == ERROR_FILE_NOT_FOUND {
        String::new()
    } else {
        unsafe { RegCloseKey(key) };
        return Err(format!(
            "cannot query HKCU\\Environment\\Path (Windows error {query})"
        ));
    };

    let current = OsString::from(&existing);
    if path_list_contains(&current, bin_dir) {
        unsafe { RegCloseKey(key) };
        update_process_path(bin_dir);
        return Ok(false);
    }

    let value = append_path_entry(&existing, &bin_dir.display().to_string());
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let data_bytes = unsafe {
        std::slice::from_raw_parts(
            data.as_ptr().cast::<u8>(),
            data.len() * std::mem::size_of::<u16>(),
        )
    };
    let value_type = if kind == REG_EXPAND_SZ {
        REG_EXPAND_SZ
    } else {
        REG_SZ
    };
    // SAFETY: `data_bytes` is a valid byte buffer for its length.
    let status = unsafe {
        RegSetValueExW(
            key,
            value_name.as_ptr(),
            0,
            value_type,
            data_bytes.as_ptr(),
            u32::try_from(data_bytes.len()).unwrap_or(u32::MAX),
        )
    };
    unsafe { RegCloseKey(key) };
    if status != 0 {
        return Err(format!(
            "cannot write HKCU\\Environment\\Path (Windows error {status})"
        ));
    }

    let environment = wide("Environment");
    let mut result = 0usize;
    // SAFETY: HWND_BROADCAST and a NUL-terminated "Environment" string are valid.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            1000,
            &mut result,
        );
    }
    Ok(true)
}

#[cfg(unix)]
fn update_user_path(bin_dir: &Path) -> Result<bool, String> {
    use std::io::Write as _;

    let home = env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    let line = format!("export PATH=\"{}:$PATH\"", bin_dir.display());
    let marker = "# impresario: PATH setup";
    let candidates = [
        PathBuf::from(&home).join(".profile"),
        PathBuf::from(&home).join(".bashrc"),
        PathBuf::from(&home).join(".zshrc"),
    ];
    let mut updated = false;
    for path in candidates {
        if !path.exists() && path.file_name().is_some_and(|name| name != ".profile") {
            continue;
        }
        let existing = fs::read_to_string(&path).unwrap_or_default();
        if existing.contains(marker) {
            continue;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
        writeln!(file, "\n{marker}\n{line}")
            .map_err(|error| format!("cannot update {}: {error}", path.display()))?;
        updated = true;
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_list_detects_existing_directory() {
        let value = env::join_paths([Path::new("/one"), Path::new("/two")]).expect("path list");
        assert!(path_list_contains(&value, Path::new("/two")));
        assert!(!path_list_contains(&value, Path::new("/three")));
    }

    #[test]
    fn appending_a_path_entry_preserves_the_existing_separator() {
        assert_eq!(append_path_entry("one", "bin"), "one;bin");
        assert_eq!(append_path_entry("one;", "bin"), "one;bin");
        assert_eq!(append_path_entry("", "bin"), "bin");
        assert_eq!(append_path_entry("  ", "bin"), "bin");
    }
}
