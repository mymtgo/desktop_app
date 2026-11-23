//! The app's only source of randomness (07 spec ruling 10): device ids, the
//! PKCE verifier and state, multipart boundaries, manual league tokens and
//! the fallback key file all come from here. Should `getrandom` ever be
//! refused, this one function is swapped for std-only bytes.

/// `len` bytes from the operating system's random source.
pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    // The OS source does not fail on any supported platform; if it ever
    // did, no id or key could be made safely, so stopping is right.
    getrandom::fill(&mut bytes).expect("the operating system's random source works");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_bytes_have_the_asked_length_and_differ() {
        assert_eq!(random_bytes(0), Vec::<u8>::new());
        let first = random_bytes(32);
        let second = random_bytes(32);
        assert_eq!(first.len(), 32);
        assert_ne!(first, second);
    }
}
