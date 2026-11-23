//! Replays (replays spec): the replay window and its view, sharing a
//! match's replay on mymtgo.com and switching the link off. The snapshot
//! rules live in the `replay` crate, the request shapes in `mymtgo-api`;
//! this module reads the stored rows, sends through `ApiState` and writes
//! the link through the tracker.

pub mod commands;
pub mod share;
pub mod view;
pub mod window;
