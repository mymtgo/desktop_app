//! A share link opening on one game of the match.

/// 0.x `ShareReplay::linkToGame`: the match link plus `?game=N`, or
/// `&game=N` when the link already has a query.
pub fn link_to_game(url: &str, game_number: u32) -> String {
    let joiner = if url.contains('?') { '&' } else { '?' };
    format!("{url}{joiner}game={game_number}")
}
