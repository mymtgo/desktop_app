//! TypeScript bindings for the commands and events (spec 6, `tauri-specta`).

use tauri_specta::{collect_commands, collect_events};

use crate::accounts;
use crate::api;
use crate::archetypes;
use crate::cards;
use crate::commands;
use crate::dashboard;
use crate::decks;
use crate::events::{
    AccountChanged, ApiChanged, AppNotification, ArchetypesChanged, CardsChanged, DecksChanged,
    GuidesChanged, HistoryImportsChanged, HistoryScanChanged, ImportProgressChanged,
    LeaguesChanged, LimitedChanged, MatchArchetypesChanged, MatchAttention, MatchDetected,
    MatchPreviewed, MatchRemoved, MatchUpdated, MetagameSharesReady, OfflineModeChanged,
    RankChanged, SettingsChanged, StatusChanged, TrackerRestarting, UpdateStatusChanged,
};
use crate::external;
use crate::first_run;
use crate::guides;
use crate::history_import;
use crate::leagues;
use crate::limited;
use crate::manual;
use crate::matches;
use crate::opponents;
use crate::overlay;
use crate::overlay::events::{
    DraftNotesChanged, GameOverlayChanged, LeagueOverlayChanged, OverlayPublishChanged,
};
use crate::paths;
use crate::replay;
use crate::replay::window::ReplayGameChanged;
use crate::support;
use crate::whats_new;

pub fn builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::app_info,
            commands::open_log_folder,
            commands::list_matches,
            commands::get_status,
            commands::get_settings,
            commands::save_settings,
            commands::set_watcher_active,
            commands::set_beta_updates,
            crate::recorder::commands::recorder_status,
            crate::recorder::commands::accept_recorder_notice,
            crate::recorder::commands::set_recorder_enabled,
            crate::recorder::commands::retry_recorder,
            crate::recorder::commands::open_recorder_folder,
            accounts::accounts_list,
            accounts::switch_account,
            accounts::set_account_tracked,
            crate::integrations::integrations,
            crate::integrations::set_integration,
            crate::platform::platform_context,
            crate::platform::switch_platform,
            crate::platform::live_matches,
            crate::rank::deck_rank,
            commands::choose_log_folder,
            paths::folder_statuses,
            paths::choose_folder,
            commands::pick_legacy_db,
            commands::report_ui_error,
            commands::get_update_status,
            commands::check_for_updates,
            commands::install_update,
            commands::get_legacy_import,
            commands::choose_legacy_db,
            commands::start_legacy_import,
            commands::launch_at_login_changed,
            crate::console::commands::console_run,
            crate::console::commands::console_cancel,
            crate::console::commands::console_schema,
            crate::console::commands::console_parameters,
            crate::console::commands::console_export,
            crate::console::commands::console_saved_list,
            crate::console::commands::console_saved_put,
            crate::console::commands::console_saved_delete,
            crate::console::commands::open_database_folder,
            crate::console::window::open_console_window,
            whats_new::whats_new_on_start,
            first_run::first_run_state,
            first_run::choose_first_run,
            first_run::dismiss_first_run,
            decks::list_decks,
            decks::deck_header,
            decks::deck_dashboard,
            decks::deck_decklist,
            decks::deck_matches,
            decks::deck_cover_options,
            decks::deck_cover_settings,
            decks::rename_deck,
            decks::set_deck_identity,
            decks::set_deck_cover,
            decks::cover::set_deck_cover_image,
            decks::cover::remove_deck_cover,
            decks::archive_deck,
            decks::restore_deck,
            decks::sync_decks,
            decks::archetype_pages::set_deck_archetype,
            decks::archetype_pages::update_deck_archetype,
            decks::archetype_pages::deck_matchups,
            decks::archetype_pages::deck_matchup_detail,
            decks::archetype_pages::deck_game_stats,
            decks::archetype_pages::deck_archetype_shell,
            decks::archetype_pages::archetype_deck_matches,
            decks::archetype_pages::listed_match_decks,
            decks::archetype_pages::archetype_deck_matchups,
            decks::archetype_pages::archetype_deck_trends,
            decks::archetype_pages::archetype_deck_card_stats,
            decks::export_deck_dek,
            decks::screenshot::deck_screenshot_data,
            decks::deck_sync_list,
            decks::set_deck_cloud_sync,
            leagues::list_leagues,
            leagues::deck_leagues,
            leagues::deck_tournaments,
            leagues::deck_league_panels,
            leagues::league_available_matches,
            leagues::create_manual_league,
            leagues::link_league_match,
            leagues::unlink_league_match,
            leagues::set_league_notes,
            leagues::set_league_status,
            leagues::delete_league,
            leagues::undo_delete_league,
            leagues::restore_league,
            leagues::move_match_to_league,
            leagues::match_league_targets,
            leagues::league_screenshot_data,
            limited::limited_index,
            limited::limited_run_colours,
            limited::limited_event,
            limited::limited_draft,
            limited::limited_cross_drafts,
            limited::limited_deck,
            limited::limited_cards,
            limited::limited_matches,
            limited::limited_match,
            limited::set_pick_note,
            limited::draft_notes,
            overlay::league::league_overlay,
            overlay::league::set_overlay_deck_label,
            overlay::background::choose_overlay_background,
            overlay::background::remove_overlay_background,
            overlay::commands::overlay_settings_page,
            overlay::commands::game_overlay,
            overlay::commands::fit_game_overlay,
            overlay::commands::set_game_overlay_collapsed,
            overlay::commands::overlay_archetype_options,
            overlay::commands::set_overlay_opponent_archetype,
            overlay::commands::add_overlay_note,
            overlay::commands::delete_overlay_note,
            opponents::opponents,
            dashboard::home_dashboard,
            dashboard::dashboard_picks,
            external::open_external,
            support::save_report_bundle,
            matches::match_detail,
            matches::game_log,
            matches::set_match_notes,
            matches::hide_match,
            matches::restore_match,
            matches::deleted_items,
            matches::set_match_deck,
            replay::commands::replay_view,
            replay::window::open_replay_window,
            replay::window::replay_requested_game,
            replay::commands::replay_share_state,
            replay::commands::share_replay,
            replay::commands::revoke_replay,
            matches::archetype_options,
            matches::set_opponent_archetype,
            matches::detect_archetypes,
            manual::manual_league_options,
            manual::manual_rank_gaps,
            manual::create_manual_match,
            manual::set_manual_hand,
            manual::set_manual_sideboard,
            manual::set_manual_reveals,
            manual::card_search,
            manual::match_edit_view,
            manual::correct_match,
            history_import::start_history_scan,
            history_import::history_scan_status,
            history_import::cancel_history_scan,
            history_import::history_scan_matches,
            history_import::history_scan_cards,
            history_import::import_history,
            history_import::import_history_all,
            history_import::delete_history_imports,
            history_import::history_imported_count,
            history_import::history_deck_versions,
            api::status::api_status,
            api::status::reauthenticate,
            api::signin::account_status,
            api::signin::sign_in,
            api::signin::cancel_sign_in,
            api::signin::sign_out,
            api::offline::offline_mode,
            api::offline::set_offline_mode,
            api::shipping::pending_match_reports,
            api::shipping::submit_pending_matches,
            api::sync_runner::sync_status,
            api::sync_runner::sync_now,
            api::sync_runner::sync_cloud_counts,
            cards::card_data::card_data_status,
            cards::card_data::download_card_data,
            cards::images::card_images_status,
            cards::folder::move_card_images,
            cards::folder::reset_card_images_path,
            cards::folder::choose_card_images_folder,
            cards::page::cards_page,
            cards::page::card_archetypes,
            cards::page::fetch_missing_cards,
            cards::stats::deck_card_stats,
            cards::data_url::card_image_data_url,
            cards::stats::regenerate_card_stats,
            cards::stats::set_card_stats_trust,
            archetypes::detect_deck_archetypes,
            archetypes::pending_archetype_count,
            archetypes::has_archetype_definitions,
            archetypes::manage::manage_archetypes,
            archetypes::manage::merge_candidates,
            archetypes::manage::reassign_candidates,
            archetypes::manage::archetype_form_data,
            archetypes::manage::create_form_matches,
            archetypes::manage::archetype_create_prefill,
            archetypes::manage::scan_match_cards,
            archetypes::manage::read_dek_file,
            archetypes::manage::create_archetype,
            archetypes::manage::update_archetype,
            archetypes::manage::remove_variant,
            archetypes::manage::reassign_variant,
            archetypes::manage::merge_archetype,
            archetypes::manage::unmerge_archetype,
            archetypes::manage::delete_archetype,
            archetypes::manage::export_archetype_dek,
            archetypes::metagame::metagame,
            archetypes::metagame::archetype_detail,
            archetypes::metagame::archetype_decklists,
            archetypes::catalog::archetype_catalog,
            archetypes::offline::download_offline_archetypes,
            archetypes::offline::redetect_offline_matches,
            archetypes::has_offline_archetypes,
            archetypes::catalog::hold_archetype,
            archetypes::metagame::save_image,
            guides::deck_guides,
            guides::guide_editor,
            guides::archetype_guides,
            guides::guide_deck_options,
            guides::create_guide,
            guides::save_guide_cards,
            guides::delete_guide,
            guides::add_note,
            guides::delete_note,
        ])
        .events(collect_events![
            MatchUpdated,
            MatchPreviewed,
            MatchRemoved,
            MatchAttention,
            StatusChanged,
            TrackerRestarting,
            SettingsChanged,
            UpdateStatusChanged,
            ImportProgressChanged,
            DecksChanged,
            AccountChanged,
            RankChanged,
            LeaguesChanged,
            LimitedChanged,
            HistoryScanChanged,
            HistoryImportsChanged,
            ApiChanged,
            OfflineModeChanged,
            AppNotification,
            CardsChanged,
            GameOverlayChanged,
            DraftNotesChanged,
            LeagueOverlayChanged,
            OverlayPublishChanged,
            ReplayGameChanged,
            MatchArchetypesChanged,
            MatchDetected,
            ArchetypesChanged,
            MetagameSharesReady,
            GuidesChanged,
        ])
}

#[cfg(test)]
mod tests {
    /// Writes `ui/src/bindings.ts` (run by `cargo test`). CI fails when the
    /// committed file differs, so commit it after changing a command or event.
    #[test]
    fn export_bindings() {
        super::builder()
            .export(
                specta_typescript::Typescript::default(),
                "../ui/src/bindings.ts",
            )
            .expect("export the TypeScript bindings");
    }

    /// The command names in `ui/src/bindings.ts`, as Tauri invokes them.
    fn command_names() -> Vec<String> {
        let bindings = std::fs::read_to_string("../ui/src/bindings.ts").expect("read bindings.ts");
        let mut names = Vec::new();
        for call in bindings.split("__TAURI_INVOKE").skip(1) {
            let Some(start) = call.find("(\"") else {
                continue;
            };
            let rest = &call[start + 2..];
            let Some(end) = rest.find('"') else {
                continue;
            };
            names.push(rest[..end].to_string());
        }
        names
    }

    /// The permissions a capability file grants.
    fn granted(file: &str) -> Vec<String> {
        let text =
            std::fs::read_to_string(format!("capabilities/{file}")).expect("read capability");
        let json: serde_json::Value = serde_json::from_str(&text).expect("capability JSON");
        json["permissions"]
            .as_array()
            .expect("a permissions list")
            .iter()
            .filter_map(|permission| permission.as_str().map(str::to_string))
            .collect()
    }

    /// Every command is checked against the window's capability (#389), so
    /// a command the main window is not granted fails there.
    #[test]
    fn the_main_window_is_granted_every_command() {
        let names = command_names();
        assert!(names.len() > 100, "found {} commands", names.len());
        let granted = granted("default.json");
        let missing: Vec<String> = names
            .iter()
            .map(|name| format!("allow-{}", name.replace('_', "-")))
            .filter(|permission| !granted.contains(permission))
            .collect();
        assert!(
            missing.is_empty(),
            "default.json does not grant {missing:?}"
        );
    }
}
