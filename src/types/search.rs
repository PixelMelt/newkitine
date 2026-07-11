#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchScope {
    Global,
    Room(String),
    Buddies,
    User(String),
}
