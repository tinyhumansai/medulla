//! Command-side UI behavior, split by the state each action owns.
//!
//! [`dispatch`] handles general runtime, prompt, clipboard, slash-command, and
//! settings actions. [`changes`] owns mutations of the Changes review model.

mod changes;
mod dispatch;
