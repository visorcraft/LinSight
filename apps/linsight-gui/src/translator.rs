// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! FFI wrapper around the C++ helper in `translator.cpp` that loads
//! a `.qm` catalog for the system locale and installs it on the
//! QCoreApplication. cxx-qt-lib 0.8 doesn't bind `QTranslator`, so we
//! invoke the helper directly.

unsafe extern "C" {
    fn linsight_install_system_translator() -> i32;
}

/// Try to install a Qt translator for the system locale.
/// Returns `true` if a `.qm` catalog was found and installed,
/// `false` if no catalog was available (Qt will fall back to source
/// strings, which are English in LinSight).
pub fn install_system_translator() -> bool {
    unsafe { linsight_install_system_translator() != 0 }
}
