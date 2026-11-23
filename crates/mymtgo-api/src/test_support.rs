//! Helpers shared by this crate's unit tests.

use std::io::Read;

use crate::request::ApiResponse;

/// A response with `status` and `body` and no headers.
pub fn response(status: u16, body: &str) -> ApiResponse {
    ApiResponse {
        status,
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    }
}

/// `bytes` decompressed from gzip.
pub fn gunzip(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .expect("gzip");
    out
}
