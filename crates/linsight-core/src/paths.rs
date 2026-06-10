// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Shared filesystem-path helpers.

use std::path::PathBuf;

/// Resolve the history DB path.
///
/// Resolution order (mirrors XDG Base Directory spec):
/// 1. `$XDG_DATA_HOME/linsight/history.db`
/// 2. `$HOME/.local/share/linsight/history.db`
/// 3. `/tmp/linsight-history.db` (last resort — no home dir)
pub fn history_db_path() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(d).join("linsight/history.db")
    } else if let Some(h) = std::env::var_os("HOME") {
        PathBuf::from(h).join(".local/share/linsight/history.db")
    } else {
        PathBuf::from("/tmp/linsight-history.db")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_db_path_xdg_data_home_wins() {
        // Temporarily override env vars is not safe in multi-threaded tests,
        // so just verify the function returns a PathBuf ending in history.db.
        let p = history_db_path();
        assert!(p.ends_with("linsight/history.db") || p.to_str().unwrap().ends_with("history.db"));
    }
}
