// Rust guideline compliant 2026-02-16

//! Where Windows says programs are: Start Menu shortcuts, the registry's
//! uninstall entries, a file picker, and a program's own name.
//!
//! Everything here is a thin, failure-tolerant wrapper over a Win32 API: a
//! shortcut that won't resolve, a registry key that won't open or a version
//! resource that isn't there is simply skipped. None of it is on a hot path;
//! [`start_menu_targets`] and [`registry_install_folders`] are called once
//! per detection pass, and [`pick_executable`] blocks on a modal dialog, so
//! the caller runs it on a thread of its own.
//!
//! COM is initialised apartment-threaded on the calling thread for the
//! duration of each call and released after, which is what the shell
//! interfaces expect and keeps the caller free of COM lifetime concerns.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    IPersistFile, STGM_READ,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW,
    RegOpenKeyExW,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FILEMUSTEXIST, FOS_NOCHANGEDIR, FileOpenDialog, IFileOpenDialog, IShellItem, IShellLinkW,
    SHCreateItemFromParsingName, SIGDN_FILESYSPATH, ShellLink,
};
use windows::core::{HSTRING, Interface, PWSTR, w};

/// Longest path a shortcut target or registry value is read into, in
/// UTF-16 units. Four times `MAX_PATH`: long-path-aware installs exist, and
/// a buffer too small is a silently skipped program.
const WIDE_BUFFER: usize = 1040;

/// The registry keys installers register themselves under, relative to the
/// hive: the native view, the 32-bit view on 64-bit Windows, and the
/// per-user view for installs that never asked for elevation.
const UNINSTALL_KEYS: &[(HKEY, &str)] = &[
    (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
    (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
    (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
];

/// Keeps COM initialised on this thread for as long as it lives.
///
/// `CoInitializeEx` must be balanced by `CoUninitialize` whether it returned
/// `S_OK` or `S_FALSE` (already initialised); only a failure is left alone.
struct ComScope {
    initialised: bool,
}

impl ComScope {
    fn new() -> Self {
        // SAFETY: plain COM initialisation with no reserved pointer; the
        // matching uninitialise is in `Drop`, on the same thread.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self { initialised: result.is_ok() }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.initialised {
            // SAFETY: balances the successful `CoInitializeEx` in `new`, on
            // the thread that made it.
            unsafe { CoUninitialize() };
        }
    }
}

/// The target of every `.lnk` under the user's and the machine's Start Menu.
///
/// Targets are returned as written, unchecked: a shortcut can point at a
/// program since uninstalled, so callers test `is_file` themselves.
#[must_use]
pub fn start_menu_targets() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        roots.push(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(program_data) = std::env::var_os("ProgramData") {
        roots.push(PathBuf::from(program_data).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    let mut shortcuts = Vec::new();
    for root in roots {
        collect_shortcuts(&root, &mut shortcuts, 0);
    }

    let _com = ComScope::new();
    // SAFETY: a standard in-process COM object; the interface is dropped
    // before the scope releases COM.
    let link: Option<IShellLinkW> = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok();
    let Some(link) = link else {
        return Vec::new();
    };
    let Ok(file) = link.cast::<IPersistFile>() else {
        return Vec::new();
    };
    shortcuts.iter().filter_map(|shortcut| shortcut_target(&link, &file, shortcut)).collect()
}

/// How deep under a Start Menu root to look. Programs put their shortcuts at
/// the top or in one folder; anything deeper is a suite's own tree.
const SHORTCUT_DEPTH: usize = 3;

/// Gathers `.lnk` files under `dir`, a few folders deep.
fn collect_shortcuts(dir: &Path, into: &mut Vec<PathBuf>, depth: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < SHORTCUT_DEPTH {
                collect_shortcuts(&path, into, depth + 1);
            }
        } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("lnk")) {
            into.push(path);
        }
    }
}

/// Resolves one shortcut to the path it points at.
fn shortcut_target(link: &IShellLinkW, file: &IPersistFile, shortcut: &Path) -> Option<PathBuf> {
    let name = HSTRING::from(shortcut.as_os_str());
    let mut target = [0u16; WIDE_BUFFER];
    // SAFETY: `Load` reads the file named by a valid wide string; `GetPath`
    // writes into a buffer whose length it is told, with no find-data
    // requested (null is documented as allowed) and flags of zero, which asks
    // for the target as the shell would resolve it.
    unsafe {
        file.Load(&name, STGM_READ).ok()?;
        link.GetPath(&mut target, std::ptr::null_mut(), 0).ok()
    }?;
    let path = wide_to_string(&target);
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Every uninstall entry's display name with the folders it points at.
///
/// Each entry contributes its `InstallLocation` and the folder holding its
/// `DisplayIcon`, whichever it has. Entries with no display name are
/// skipped: nothing could be matched against them.
#[must_use]
pub fn registry_install_folders() -> Vec<(String, Vec<PathBuf>)> {
    let mut found = Vec::new();
    for (hive, key) in UNINSTALL_KEYS {
        let Some(root) = open_key(*hive, key) else {
            continue;
        };
        for subkey in subkeys(&root) {
            let Some(name) = string_value(&root, &subkey, "DisplayName") else {
                continue;
            };
            let mut folders = Vec::new();
            if let Some(location) = string_value(&root, &subkey, "InstallLocation")
                && !location.is_empty()
            {
                folders.push(PathBuf::from(location));
            }
            if let Some(icon) = string_value(&root, &subkey, "DisplayIcon") {
                // `DisplayIcon` is `path\to\thing.exe` or `path\to\file,0`.
                let path = icon.split(',').next().unwrap_or_default().trim_matches('"');
                if let Some(dir) = Path::new(path).parent() {
                    folders.push(dir.to_path_buf());
                }
            }
            if !folders.is_empty() {
                found.push((name, folders));
            }
        }
    }
    found
}

/// An open registry key, closed on drop.
struct OpenKey(HKEY);

impl Drop for OpenKey {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open_key` and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// Opens `key` under `hive` for reading.
fn open_key(hive: HKEY, key: &str) -> Option<OpenKey> {
    let name = HSTRING::from(key);
    let mut handle = HKEY(std::ptr::null_mut());
    // SAFETY: the out-pointer is a valid `HKEY` on this stack frame; the
    // key name is a valid wide string.
    let status = unsafe { RegOpenKeyExW(hive, &name, None, KEY_READ, &raw mut handle) };
    (status == ERROR_SUCCESS).then_some(OpenKey(handle))
}

/// The names of every subkey of `key`.
fn subkeys(key: &OpenKey) -> Vec<String> {
    let mut names = Vec::new();
    let mut index = 0;
    loop {
        let mut buffer = [0u16; WIDE_BUFFER];
        let mut length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        // SAFETY: the buffer and its length travel together, and the
        // remaining arguments are the documented "not wanted" values.
        let status = unsafe {
            RegEnumKeyExW(key.0, index, Some(PWSTR(buffer.as_mut_ptr())), &raw mut length, None, None, None, None)
        };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(wide_to_string(&buffer));
        index += 1;
    }
    names
}

/// A string value under `subkey` of `key`, if it exists and is a string.
fn string_value(key: &OpenKey, subkey: &str, value: &str) -> Option<String> {
    let subkey = HSTRING::from(subkey);
    let value = HSTRING::from(value);
    let mut buffer = [0u16; WIDE_BUFFER];
    let mut size = u32::try_from(std::mem::size_of_val(&buffer)).unwrap_or(u32::MAX);
    // SAFETY: the data pointer and byte size describe the same buffer;
    // `RRF_RT_REG_SZ` makes the call fail rather than write anything else.
    let status = unsafe {
        RegGetValueW(key.0, &subkey, &value, RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&raw mut size))
    };
    (status == ERROR_SUCCESS).then(|| wide_to_string(&buffer))
}

/// Shows the system file picker and returns the executable chosen.
///
/// Blocks until the dialog closes, so it belongs on a worker thread. `None`
/// when the driver cancels or the dialog can't be shown. `initial` is the
/// program the picker opens beside, if the row already had one.
#[must_use]
pub fn pick_executable(initial: Option<&Path>) -> Option<PathBuf> {
    let _com = ComScope::new();
    // SAFETY: standard IFileOpenDialog use — every interface pointer comes
    // from COM and is released by drop; the filter strings are static wide
    // literals that outlive the call; the result string is freed with
    // `CoTaskMemFree` after being copied.
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [
            COMDLG_FILTERSPEC { pszName: w!("Programs"), pszSpec: w!("*.exe") },
            COMDLG_FILTERSPEC { pszName: w!("All files"), pszSpec: w!("*.*") },
        ];
        dialog.SetFileTypes(&filters).ok()?;
        dialog.SetTitle(w!("Choose a program to start")).ok()?;
        if let Ok(options) = dialog.GetOptions() {
            let _ = dialog.SetOptions(options | FOS_FILEMUSTEXIST | FOS_NOCHANGEDIR);
        }
        if let Some(dir) = initial.and_then(Path::parent) {
            let folder: Result<IShellItem, _> = SHCreateItemFromParsingName(&HSTRING::from(dir.as_os_str()), None);
            if let Ok(folder) = folder {
                let _ = dialog.SetFolder(&folder);
            }
        }
        // A cancelled dialog is an error return, and the only one worth
        // nothing to say about.
        dialog.Show(None).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.as_ptr().cast_const().cast()));
        path
    }
}

/// A program's own description from its version resource, if it has one.
///
/// `FileDescription` from the first translation in the resource — what
/// Explorer shows as the file's "description" — which is a better name for
/// a row than `deck.exe`.
#[must_use]
pub fn file_description(path: &Path) -> Option<String> {
    let name = HSTRING::from(path.as_os_str());
    // SAFETY: the buffer is sized by the first call and handed whole to the
    // second; each `VerQueryValueW` returns a pointer into that buffer, read
    // only while it is alive and only up to the length it reports.
    unsafe {
        let size = GetFileVersionInfoSizeW(&name, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; usize::try_from(size).ok()?];
        GetFileVersionInfoW(&name, None, size, data.as_mut_ptr().cast()).ok()?;

        let mut translation: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut length = 0u32;
        if !VerQueryValueW(data.as_ptr().cast(), w!(r"\VarFileInfo\Translation"), &raw mut translation, &raw mut length)
            .as_bool()
            || translation.is_null()
            || usize::try_from(length).is_ok_and(|bytes| bytes < std::mem::size_of::<u32>())
        {
            return None;
        }
        let pair = translation.cast::<u16>();
        let language = *pair;
        let codepage = *pair.add(1);
        let query = HSTRING::from(format!(r"\StringFileInfo\{language:04x}{codepage:04x}\FileDescription"));

        let mut text: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut chars = 0u32;
        if !VerQueryValueW(data.as_ptr().cast(), &query, &raw mut text, &raw mut chars).as_bool() || text.is_null() {
            return None;
        }
        let wide = std::slice::from_raw_parts(text.cast::<u16>(), usize::try_from(chars).ok()?);
        let description = wide_to_string(wide);
        (!description.trim().is_empty()).then(|| description.trim().to_owned())
    }
}

/// Converts a NUL-terminated (or NUL-padded) wide buffer to a `String`.
fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&unit| unit == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}
