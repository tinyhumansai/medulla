//! Unit tests for the session model, split by surface so no file exceeds the
//! repo's 500-line ceiling:
//! [`routing_tests`] covers class/transport routing and the provider capability
//! matrix; [`registry_tests`] the binding registry and turn serialization;
//! [`input_tests`] the task-frame/envelope driver seam; [`completion_tests`]
//! interactive turn-completion detection; and [`manager_tests`] the session
//! lifecycle and turn execution.

mod completion_tests;
mod input_tests;
mod manager_tests;
mod registry_tests;
mod routing_tests;
