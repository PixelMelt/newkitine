mod actor;
mod bootstrap;
mod event;
mod observation;
mod punctuation;
mod search;
mod shares;
mod transfers;
mod users;

pub use actor::Client;
pub use bootstrap::ClientBootstrap;
pub use event::{ClientEvent, SearchResult, UserInfoReceived};
pub use observation::Observation;
pub use search::SearchScope;
pub use transfers::{AbortResult, EnqueueResult, RetryResult, TransferWork};

const COMMAND_QUEUE_CAPACITY: usize = 1024;
const EVENT_QUEUE_CAPACITY: usize = 4096;
const TRANSFER_QUEUE_CAPACITY: usize = 4096;
pub const FOLDER_DOWNLOAD_FILE_LIMIT: usize = 1000;
