//! Endstep support (endstep support spec): the capture files the browser
//! extension's helper drops in the inbox are imported into the database.

pub mod host;
pub mod import;

#[cfg(test)]
mod tests;
