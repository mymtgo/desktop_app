//! The opt-in MTGO recorder (recorded replays spec): its finished `.mtgr`
//! files are imported from the recorder's folder.

pub mod commands;
pub mod import;
pub mod install;
pub mod process;
pub mod supervisor;
