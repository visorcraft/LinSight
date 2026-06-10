// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

pub mod alert_model;
pub mod dashboards_model;
pub mod hardware_model;
pub mod overview_model;
pub mod preferences_model;
pub(crate) mod rpc_worker;
pub mod workspace_handle;

pub use workspace_handle::install_workspace;
