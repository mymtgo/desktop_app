//! Pure rules for Endstep capture: the native messaging protocol shared by
//! the helper binary and the browser extension, and the capture file format.

pub mod capture;
pub mod framing;
pub mod last_seen;
pub mod message;
pub mod registration;

pub use capture::{
    CaptureError, CaptureHeader, Classification, FORMAT_VERSION, ParsedCapture, UnitKind,
    parse_capture,
};
/// The largest whole capture file the helper assembles from parts and the app
/// imports. The extension mirrors it (`extension/src/background/delivery.ts`).
pub const MAX_CAPTURE_BYTES: u64 = 512 * 1024 * 1024;

pub use framing::{FramingError, MAX_MESSAGE_BYTES, read_message, write_message};
pub use last_seen::{Connection, LastSeen, connection, last_seen_json, parse_last_seen};
pub use message::{HostReply, HostRequest, valid_unit_id};
pub use registration::{
    CHROME_EXTENSION_IDS, FIREFOX_EXTENSION_ID, Family, HOST_NAME, Os, Place, Target,
    chrome_manifest, firefox_manifest, helper_file_name, targets,
};
