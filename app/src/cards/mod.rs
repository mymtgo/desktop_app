//! Cards (sub-project 06): the `cards` worker thread and the card commands.
//!
//! - `populate`: one population run (0.x `PopulateMissingCardData`): token
//!   names from MTGO's XML, the API passes, then `CardsResolved` through
//!   the tracker.
//! - `worker`: the thread that runs population hourly, after new stubs, after
//!   a sync pull and when asked, one run at a time.
//! - `card_data`: the Settings "Card data" card's commands.
//! - `data_url`: card images as `data:` URLs for screenshots.
//! - `folder`: moving the card images folder.
//! - `images`: local card images (downloads, clearing, the Settings "Card
//!   images" card).
//! - `page`: the Cards page (grid, archetype sheet, "Fetch missing").
//! - `stats`: the deck Card Stats tab (payload, "Regenerate", trust).
//! - `legacy_images`: the copy of 0.x's image files into v1's folder after a
//!   0.x import (0.x's folder is only read).

pub mod card_data;
pub mod data_url;
pub mod folder;
pub mod images;
pub mod legacy_images;
pub mod page;
pub mod populate;
pub mod scheme;
pub mod stats;
pub mod worker;
