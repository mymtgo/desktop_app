//! Get help's report bundle (release spec section 8.1): a zip writer over
//! any `std::io::Write`, the choice of report files, the summary text and
//! the report's file name. Pure: file listings, times and readers come in,
//! bytes go out to a writer the app opens.

pub mod report;
pub mod zip;

pub use report::{
    APP_LOG_LIMIT, FileRef, ReportEntry, SUMMARY_NAME, SummaryInput, is_app_log_name,
    is_recorder_log_name, pick_report_files, report_file_name, summary_text, write_report,
};
pub use zip::{DosTime, ZipError, ZipWriter};
