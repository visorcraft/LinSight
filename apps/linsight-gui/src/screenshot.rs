// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! FFI wrapper around `screenshot.cpp`. Arms a QTimer that grabs the
//! first visible `QQuickWindow` after `delay_ms` and writes it as PNG
//! to `path`, then exits the event loop. Used by the
//! `--screenshot <path>` CLI flag to bypass Wayland compositor caching
//! on unfocused windows.

use std::ffi::CString;
use std::path::Path;

unsafe extern "C" {
    fn linsight_arm_screenshot(path_utf8: *const std::os::raw::c_char, delay_ms: i32);
}

pub fn arm(path: &Path, delay_ms: i32) {
    let s = path.to_string_lossy();
    let Ok(c) = CString::new(s.as_bytes()) else {
        tracing::warn!("screenshot path contains a NUL — skipping");
        return;
    };
    unsafe { linsight_arm_screenshot(c.as_ptr(), delay_ms) };
}
