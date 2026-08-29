//! Shared types and the pieces both binaries need.

/// The one place the product name lives. Used for config paths, the IPC socket
/// name, the virtual input device name, and notifications, so a rename touches
/// this constant plus packaging rather than the whole tree.
pub const APP_NAME: &str = "sensor";

pub mod audio;
pub mod hotkey;
pub mod output;
pub mod stt;
