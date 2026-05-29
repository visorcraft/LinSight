// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::sync::Arc;

use crate::client::ClientHandle;

/// Process-wide shared state for the GUI. Holds the daemon client.
pub struct Workspace {
    client: ClientHandle,
}

impl Workspace {
    pub fn new(client: ClientHandle) -> Self {
        Self { client }
    }

    pub fn client(&self) -> Arc<crate::client::Client> {
        Arc::clone(&self.client)
    }
}
