//! The data console (spec `2026-11-20-data-console-design.md`, section 5):
//! a locked-down read-only connection that runs one user statement at a
//! time, with a row cap, a timeout, a cancel flag and named parameters.

mod connection;
mod export;
mod run;
mod schema;
pub mod views;

pub use connection::open_console;
pub use export::{ExportFormat, export};
pub use run::{
    ConsoleError, ConsoleParam, ConsoleQuery, ConsoleResult, ConsoleValue, for_each_row,
    parameters, run_query,
};
pub use schema::{ConsoleColumn, ConsoleObject, ObjectKind, schema};
