mod connection;
mod file;
mod interests;
mod settings;
mod share;
mod transfer;
mod user;

pub use connection::ConnectionType;
pub use file::{FileAttributes, FileInfo, FolderContents, UINT32_LIMIT};
pub use interests::Recommendations;
pub use settings::{
    DEFAULT_BANNED_MESSAGE, DEFAULT_MAX_SEARCH_RESPONSES, DEFAULT_MAX_SEARCH_RESULTS,
    DEFAULT_MIN_SEARCH_CHARS, DEFAULT_QUEUE_FILE_LIMIT, DEFAULT_SERVER, DEFAULT_UPLOAD_SLOTS,
    DEFAULT_UPLOADS_PER_USER, FilterLevel, LoginConfig, RuntimeConfig, SearchConfig,
    TransferConfig,
};
pub use share::SharedFolder;
pub use transfer::{TransferDirection, TransferId, TransferSnapshot, TransferStatus};
pub use user::{Restriction, SimilarUser, UserStats, UserStatus};
