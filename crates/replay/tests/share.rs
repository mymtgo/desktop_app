//! The share and switch off messages (0.x `ShareReplayController` and
//! `RevokeReplayShareController`, replays spec section 8; the sign-in
//! sentences name the Account page, ruling 10).

use replay::{RevokeFailure, ShareFailure};

#[test]
fn every_failure_message_matches_0x() {
    let host = "mymtgo.test";
    let share = [
        (
            ShareFailure::NotSignedIn,
            "Sign in on the Account page to share replays.",
        ),
        (
            ShareFailure::Offline,
            "Sharing needs a connection. Turn off offline mode first.",
        ),
        (
            ShareFailure::Supporter,
            "Sharing replays is a supporter feature.",
        ),
        (
            ShareFailure::NotClaimed,
            "Link your MTGO account to your mymtgo.test account first, so the replay can be filed under your player name. We could not confirm your claim to it.",
        ),
        (ShareFailure::TooLarge, "This game is too large to share."),
        (
            ShareFailure::Invalid,
            "This replay could not be prepared for sharing.",
        ),
        (
            ShareFailure::AccountUnknown,
            "Your MTGO account is not linked yet. Play a game with the app running, then try again.",
        ),
        (
            ShareFailure::NameSurvived,
            "Couldn't hide your opponent's name, so this replay was not shared.",
        ),
        (
            ShareFailure::Unreachable,
            "Could not reach mymtgo.test. Try again in a moment.",
        ),
        (
            ShareFailure::Server,
            "mymtgo.test could not share this replay right now. Try again in a moment.",
        ),
    ];
    for (failure, message) in share {
        assert_eq!(failure.message(host), message, "{failure:?}");
    }

    let revoke = [
        (
            RevokeFailure::NotSignedIn,
            "Sign in on the Account page to switch this link off.",
        ),
        (
            RevokeFailure::Offline,
            "Switching a link off needs a connection. Turn off offline mode first.",
        ),
        (
            RevokeFailure::Unreachable,
            "Could not reach mymtgo.test. The link is still on; try again in a moment.",
        ),
    ];
    for (failure, message) in revoke {
        assert_eq!(failure.message(host), message, "{failure:?}");
    }

    assert_eq!(
        ShareFailure::Unreachable.message("mymtgo.com"),
        "Could not reach mymtgo.com. Try again in a moment."
    );
}
