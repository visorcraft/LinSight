// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Thread-local handle to the shared `Workspace`. Same pattern as
//! Grexa: the workspace is installed before QML boots; QObjects
//! access it via `with_workspace`.

use std::cell::RefCell;
use std::sync::Arc;

use crate::workspace::Workspace;

thread_local! {
    static WORKSPACE: RefCell<Option<Arc<Workspace>>> = const { RefCell::new(None) };
}

pub fn install_workspace(workspace: Arc<Workspace>) {
    WORKSPACE.with(|cell| *cell.borrow_mut() = Some(workspace));
}

pub fn with_workspace<R>(f: impl FnOnce(Arc<Workspace>) -> R) -> R {
    WORKSPACE.with(|cell| {
        let binding = cell.borrow();
        let ws =
            binding.as_ref().expect("install_workspace must be called before any QObject method");
        f(Arc::clone(ws))
    })
}
