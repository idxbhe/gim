//! Stage 1 — verify the process is running with administrator privileges.
//!
//! Every `FSCTL_*` we use below (`FSCTL_GET_RETRIEVAL_POINTERS`,
//! `FSCTL_GET_VOLUME_BITMAP`, `FSCTL_MOVE_FILE`) requires the calling
//! process token to be elevated. Without elevation the calls succeed for
//! files we own but fail for files owned by `TrustedInstaller` or other
//! users — exactly the kind of partial-success state that fragments data.
//!
//! # Two-step strategy
//!
//! 1. `is_elevated()` — inspect the current process token via
//!    `OpenProcessToken` + `GetTokenInformation(TokenElevation)`. Returns
//!    `ElevationToken::Elevated` or `ElevationToken::NotElevated`.
//! 2. `request_elevation()` — if not elevated, spawn a new copy of `gim`
//!    with the same argv via `ShellExecuteW("runas", ...)`. UAC will prompt
//!    the user. Returns `Ok(true)` if a new elevated process was launched
//!    (the caller should exit immediately), `Ok(false)` if the user refused
//!    the UAC prompt, or `Err(...)` for `ShellExecuteW` failures.
//!
//! # Non-Windows
//!
//! All entry points return `NotSupportedPlatform`. We deliberately don't
//! "fake" elevation on Unix because there's no defrag API to gate anyway.

use crate::error::{GError, GResult};

/// Result of inspecting the current process token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevationToken {
    /// The process has admin rights; FSCTLs will succeed.
    Elevated,
    /// The process is running with filtered (non-admin) rights.
    NotElevated,
}

/// `TOKEN_ELEVATION` struct from `winnt.h`. The `TokenIsElevated` field is
/// a `DWORD` (u32) — non-zero means elevated.
#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct TokenElevation {
    token_is_elevated: u32,
}

#[cfg(target_os = "windows")]
#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(
        process_handle: *mut std::ffi::c_void,
        desired_access: u32,
        token_handle: *mut *mut std::ffi::c_void,
    ) -> i32;

    fn GetTokenInformation(
        token_handle: *mut std::ffi::c_void,
        token_information_class: u32,
        token_information: *mut std::ffi::c_void,
        token_information_length: u32,
        return_length: *mut u32,
    ) -> i32;

    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
}

#[cfg(target_os = "windows")]
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut std::ffi::c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show_cmd: i32,
    ) -> *mut std::ffi::c_void;
}

// ── Win32 constants ──────────────────────────────────────────────────────
#[cfg(target_os = "windows")]
const TOKEN_QUERY: u32 = 0x0008;
#[cfg(target_os = "windows")]
const TOKEN_ELEVATION: u32 = 20; // TokenInformationClass::TokenElevation
#[cfg(target_os = "windows")]
const SW_SHOWNORMAL: i32 = 1;

/// Inspect the current process token.
///
/// Returns `Ok(ElevationToken::Elevated)` if the process already has admin
/// rights, `Ok(NotElevated)` otherwise. Errors here mean the Win32 API
/// itself failed (very unlikely — usually indicates a broken system).
pub fn is_elevated() -> GResult<ElevationToken> {
    #[cfg(target_os = "windows")]
    {
        // SAFETY: GetCurrentProcess returns a pseudo-handle that is always
        // valid for the lifetime of the process. We pass it to
        // OpenProcessToken, which writes a real handle into `token_handle`.
        unsafe {
            let proc = GetCurrentProcess();
            let mut token_handle: *mut std::ffi::c_void = std::ptr::null_mut();
            if OpenProcessToken(proc, TOKEN_QUERY, &mut token_handle) == 0 {
                return Err(GError::Defrag(format!(
                    "OpenProcessToken failed (Win32 error {})",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                )));
            }
            // Wrap the handle in a guard so CloseHandle runs even on early
            // return — leaking a token handle would exhaust the kernel
            // handle table over many invocations.
            let _guard = HandleGuard(token_handle);

            let mut elevation = TokenElevation { token_is_elevated: 0 };
            let mut return_length: u32 = 0;
            if GetTokenInformation(
                token_handle,
                TOKEN_ELEVATION,
                &mut elevation as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<TokenElevation>() as u32,
                &mut return_length,
            ) == 0 {
                return Err(GError::Defrag(format!(
                    "GetTokenInformation(TokenElevation) failed (Win32 error {})",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                )));
            }
            Ok(if elevation.token_is_elevated != 0 {
                ElevationToken::Elevated
            } else {
                ElevationToken::NotElevated
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // On non-Windows there's no admin token concept relevant to NTFS
        // FSCTLs — the whole command is unsupported here. We still return
        // NotElevated rather than NotSupportedPlatform so callers can build
        // a uniform "are we admin?" guard; the platform gate happens in
        // the command dispatcher.
        Ok(ElevationToken::NotElevated)
    }
}

/// Trigger a UAC prompt to re-launch `gim` with admin rights.
///
/// # Returns
///
/// - `Ok(true)` — a new elevated process was spawned. The caller should
///   exit immediately so the user sees only the elevated instance.
/// - `Ok(false)` — the user declined the UAC prompt (or `ShellExecuteW`
///   returned a sentinel indicating cancellation). The caller should
///   surface a friendly message and exit.
/// - `Err(...)` — `ShellExecuteW` itself failed in an unexpected way.
///
/// # Implementation
///
/// We re-execute the *current* binary (`std::env::current_exe`) with the
/// *current* argv (`std::env::args`), so the elevated copy runs the exact
/// same `gim defrag …` command the user typed — only with admin rights.
pub fn request_elevation() -> GResult<bool> {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

        let exe = std::env::current_exe()
            .map_err(|e| GError::Defrag(format!("current_exe: {e}")))?;
        let args: Vec<String> = std::env::args().skip(1).collect();
        // Build a Windows command-line: argv quoted per CommandLineToArgvW
        // rules. Each arg is wrapped in quotes if it contains spaces, with
        // backslash escaping for trailing backslashes inside quotes.
        let params = join_args(&args);

        let mut operation: Vec<u16> = OsStr::new("runas").encode_wide().collect();
        operation.push(0);
        let mut file: Vec<u16> = OsStr::new(exe.as_os_str()).encode_wide().collect();
        file.push(0);
        let mut params_w: Vec<u16> = OsStr::new(&params).encode_wide().collect();
        params_w.push(0);

        // SAFETY: All pointers are NUL-terminated UTF-16 buffers owned by
        // us and alive for the duration of the call. `hwnd` is null (no
        // owning window). `directory` is null (inherit CWD).
        let h = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                file.as_ptr(),
                params_w.as_ptr(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecuteW returns HINSTANCE > 32 on success, <= 32 on error.
        // The sentinel value 1223 (ERROR_CANCELLED) means "user clicked No
        // on the UAC prompt" — we treat that as Ok(false) rather than an
        // error so the caller can show a friendly message.
        let h_as_isize = h as isize;
        if h_as_isize > 32 {
            return Ok(true);
        }
        if h_as_isize == 1223 {
            return Ok(false);
        }
        Err(GError::Defrag(format!(
            "ShellExecuteW(runas) failed (code {})",
            h_as_isize
        )))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(GError::NotSupportedPlatform)
    }
}

/// Quote-and-join argv the way `cmd.exe` / `CommandLineToArgvW` expects.
///
/// We can't just `args.join(" ")` because args containing spaces would be
/// split by the shell. We follow the documented escaping: wrap in quotes
/// if the arg contains whitespace, double-up backslashes that precede a
/// quote, and double all backslashes at the very end when followed by a
/// closing quote.
fn join_args(args: &[String]) -> String {
    let mut out = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 { out.push(' '); }
        // Always quote — simpler and safe. Even args without spaces round-trip.
        out.push('"');
        // Count trailing backslashes so we can double them when the arg
        // is wrapped in quotes (otherwise the closing quote gets escaped).
        let mut backslashes = 0usize;
        for ch in arg.chars() {
            if ch == '\\' {
                backslashes += 1;
                out.push(ch);
            } else if ch == '"' {
                // Double the backslashes that preceded the quote, then
                // escape the quote itself with a backslash.
                for _ in 0..backslashes { out.push('\\'); }
                out.push('\\');
                out.push('"');
                backslashes = 0;
            } else {
                backslashes = 0;
                out.push(ch);
            }
        }
        // Double trailing backslashes (they precede the closing quote).
        for _ in 0..backslashes { out.push('\\'); }
        out.push('"');
    }
    out
}

#[cfg(target_os = "windows")]
struct HandleGuard(*mut std::ffi::c_void);

#[cfg(target_os = "windows")]
impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { let _ = CloseHandle(self.0); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_args_simple() {
        assert_eq!(join_args(&["defrag".into(), "cp2077".into()]),
                   "\"defrag\" \"cp2077\"");
    }

    #[test]
    fn join_args_with_spaces() {
        // Per Windows CommandLineToArgvW rules: only *trailing* backslashes
        // (those that immediately precede the closing quote) get doubled.
        // Middle backslashes (followed by non-quote chars) stay as-is.
        // "C:\Program Files\Game" has no trailing backslash → middle `\`
        // stays single.
        let s = join_args(&["defrag".into(), "C:\\Program Files\\Game".into()]);
        assert_eq!(s, "\"defrag\" \"C:\\Program Files\\Game\"");
    }

    #[test]
    fn join_args_with_trailing_backslash() {
        // Trailing backslash gets doubled because it precedes the closing quote.
        let s = join_args(&["defrag".into(), "C:\\Games\\".into()]);
        assert_eq!(s, "\"defrag\" \"C:\\Games\\\\\"");
    }

    #[test]
    fn join_args_with_embedded_quote() {
        let s = join_args(&["--msg".into(), "hello \"world\"".into()]);
        assert!(s.contains("\"hello \\\"world\\\"\""));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn is_elevated_returns_not_elevated_off_windows() {
        assert_eq!(is_elevated().unwrap(), ElevationToken::NotElevated);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn request_elevation_unsupported_off_windows() {
        assert!(matches!(request_elevation(), Err(GError::NotSupportedPlatform)));
    }
}
