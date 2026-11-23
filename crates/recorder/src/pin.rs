//! The recorder build MyMTGO is allowed to download and run.

/// Where release files are hosted.
const RELEASE_HOST: &str = "https://releases.mymtgo.com/mtgr-record";

/// One file of the pinned build and its expected SHA-256 (lowercase hex).
pub struct PinnedFile {
    pub name: &'static str,
    pub sha256: &'static str,
}

/// The pinned recorder build.
pub struct Pin {
    pub version: &'static str,
    pub consent_version: u32,
    pub exe: &'static str,
    pub files: &'static [PinnedFile],
}

/// mtgr-record 0.2.0 as uploaded to the release host: the hashes are its `SHA256SUMS`.
/// A new build gets a new version and new hashes; an empty hash blocks the download.
pub const PIN: Pin = Pin {
    version: "0.2.0",
    consent_version: 1,
    exe: "mtgr-record.exe",
    files: &[
        PinnedFile {
            name: "mtgr-record.exe",
            sha256: "51da3151b828383bf2da1e5d7c91a1afa906af4f642ecf9b768ade1f19cea3a0",
        },
        PinnedFile {
            name: "LICENSE",
            sha256: "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
        },
        PinnedFile {
            name: "NOTICE",
            sha256: "cdb77ae990f2e822936cb7397ca671ab2b6091f10d3796acab03f3b4dadc57cf",
        },
        PinnedFile {
            name: "THIRD-PARTY-NOTICES.md",
            sha256: "db3b2ac549cbb1a9ec308f8783ec7eeceb48c922c3584c4db6740cb2c657468c",
        },
        PinnedFile {
            name: "README.txt",
            sha256: "d9a791a5c4d22c63e53353e4ba1c088bfe1a9ae747bd92f7fb3a67ba3bbe686d",
        },
    ],
};

/// True while any file has no hash: nothing can be verified, so nothing downloads.
pub fn download_blocked(pin: &Pin) -> bool {
    pin.files.iter().any(|file| file.sha256.is_empty())
}

/// The download URL of one pinned file.
pub fn file_url(pin: &Pin, file: &PinnedFile) -> String {
    format!("{RELEASE_HOST}/{}/{}", pin.version, file.name)
}
