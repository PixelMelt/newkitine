use super::compress::{compress, decompress};
use super::wire::{MessageReader, MessageWriter, ProtocolError};
use crate::types::{FileAttributes, FileInfo, FolderContents, TransferDirection};

const MAX_SHARES_SIZE: usize = 2147483648;
const MAX_RESULTS_SIZE: usize = 134217728;

#[derive(Debug, Clone, PartialEq)]
pub enum PeerMessage {
    SharedFileListRequest,
    SharedFileListResponse {
        shares: Vec<FolderContents>,
        unknown: u32,
        private_shares: Vec<FolderContents>,
    },
    FileSearchRequest { token: u32, search_term: String },
    FileSearchResponse {
        username: String,
        token: u32,
        results: Vec<FileInfo>,
        free_upload_slots: bool,
        upload_speed: u32,
        queue_size: u32,
        unknown: u32,
        private_results: Vec<FileInfo>,
    },
    UserInfoRequest,
    UserInfoResponse {
        description: String,
        picture: Option<Vec<u8>>,
        total_uploads: u32,
        queue_size: u32,
        slots_available: bool,
        upload_allowed: Option<u32>,
    },
    FolderContentsRequest { token: u32, directory: String, legacy_client: bool },
    FolderContentsResponse { token: u32, directory: String, folders: Vec<FolderContents> },
    TransferRequest {
        direction: TransferDirection,
        token: u32,
        file: String,
        filesize: Option<u64>,
    },
    TransferResponse { token: u32, allowed: bool, reason: Option<String>, filesize: Option<u64> },
    PlaceholdUpload { file: String },
    QueueUpload { file: String, legacy_client: bool },
    PlaceInQueueResponse { filename: String, place: u32 },
    UploadFailed { file: String },
    UploadDenied { file: String, reason: String },
    PlaceInQueueRequest { file: String, legacy_client: bool },
    UploadQueueNotification,
    UnknownPeerMessage,
}

pub fn read_file_attributes(r: &mut MessageReader) -> Result<FileAttributes, ProtocolError> {
    let mut attrs = FileAttributes::default();
    let num = r.read_u32()?;
    for _ in 0..num {
        let attr_id = r.read_u32()?;
        let value = r.read_u32()?;
        match attr_id {
            0 => attrs.bitrate = Some(value),
            1 => attrs.length = Some(value),
            2 => attrs.vbr = Some(value),
            4 => attrs.sample_rate = Some(value),
            5 => attrs.bit_depth = Some(value),
            _ => {}
        }
    }
    Ok(attrs)
}

fn write_file_attributes(w: &mut MessageWriter, attrs: &FileAttributes) {
    let pairs = [
        (0u32, attrs.bitrate),
        (1, attrs.length),
        (2, attrs.vbr),
        (4, attrs.sample_rate),
        (5, attrs.bit_depth),
    ];
    let count = pairs.iter().filter(|(_, v)| v.is_some()).count();
    w.write_u32(count as u32);
    for (attr_id, value) in pairs {
        if let Some(value) = value {
            w.write_u32(attr_id);
            w.write_u32(value);
        }
    }
}

fn read_file_entry(r: &mut MessageReader, backslash_name: bool) -> Result<FileInfo, ProtocolError> {
    let code = r.read_u8()?;
    let mut name = r.read_string()?;
    if backslash_name {
        name = name.replace('/', "\\");
    }
    let size = r.read_file_size()?;
    let ext_len = r.read_u32()? as usize;
    r.skip(ext_len)?;
    let attributes = read_file_attributes(r)?;
    Ok(FileInfo { code, name, size, attributes })
}

fn read_file_list(r: &mut MessageReader, backslash_name: bool) -> Result<Vec<FileInfo>, ProtocolError> {
    let num = r.read_u32()? as usize;
    let mut files = Vec::with_capacity(num.min(65536));
    for _ in 0..num {
        files.push(read_file_entry(r, backslash_name)?);
    }
    if files.len() > 1 {
        files.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Ok(files)
}

fn read_folder_list(r: &mut MessageReader) -> Result<Vec<FolderContents>, ProtocolError> {
    let num = r.read_u32()? as usize;
    let mut folders = Vec::with_capacity(num.min(65536));
    for _ in 0..num {
        let directory = r.read_string()?.replace('/', "\\");
        let files = read_file_list(r, false)?;
        folders.push(FolderContents { directory, files });
    }
    if folders.len() > 1 {
        folders.sort_by(|a, b| a.directory.cmp(&b.directory));
    }
    Ok(folders)
}

fn write_file_entry(w: &mut MessageWriter, file: &FileInfo) {
    w.write_u8(file.code);
    w.write_string(&file.name);
    w.write_u64(file.size);
    w.write_u32(0);
    write_file_attributes(w, &file.attributes);
}

fn write_file_list(w: &mut MessageWriter, files: &[FileInfo]) {
    w.write_u32(files.len() as u32);
    for file in files {
        write_file_entry(w, file);
    }
}

fn write_folder_list(w: &mut MessageWriter, folders: &[FolderContents]) {
    w.write_u32(folders.len() as u32);
    for folder in folders {
        w.write_string(&folder.directory);
        write_file_list(w, &folder.files);
    }
}

impl PeerMessage {
    pub fn code(&self) -> u32 {
        match self {
            Self::SharedFileListRequest => 4,
            Self::SharedFileListResponse { .. } => 5,
            Self::FileSearchRequest { .. } => 8,
            Self::FileSearchResponse { .. } => 9,
            Self::UserInfoRequest => 15,
            Self::UserInfoResponse { .. } => 16,
            Self::FolderContentsRequest { .. } => 36,
            Self::FolderContentsResponse { .. } => 37,
            Self::TransferRequest { .. } => 40,
            Self::TransferResponse { .. } => 41,
            Self::PlaceholdUpload { .. } => 42,
            Self::QueueUpload { .. } => 43,
            Self::PlaceInQueueResponse { .. } => 44,
            Self::UploadFailed { .. } => 46,
            Self::UploadDenied { .. } => 50,
            Self::PlaceInQueueRequest { .. } => 51,
            Self::UploadQueueNotification => 52,
            Self::UnknownPeerMessage => 12547,
        }
    }

    pub fn make_payload(&self) -> Vec<u8> {
        let mut w = MessageWriter::new();
        match self {
            Self::SharedFileListRequest
            | Self::UserInfoRequest
            | Self::UploadQueueNotification
            | Self::UnknownPeerMessage => {}
            Self::SharedFileListResponse { shares, unknown, private_shares } => {
                write_folder_list(&mut w, shares);
                w.write_u32(*unknown);
                if !private_shares.is_empty() {
                    write_folder_list(&mut w, private_shares);
                }
                return compress(&w.into_bytes());
            }
            Self::FileSearchRequest { token, search_term } => {
                w.write_u32(*token);
                w.write_string(search_term);
            }
            Self::FileSearchResponse {
                username,
                token,
                results,
                free_upload_slots,
                upload_speed,
                queue_size,
                unknown,
                private_results,
            } => {
                w.write_string(username);
                w.write_u32(*token);
                write_file_list(&mut w, results);
                w.write_bool(*free_upload_slots);
                w.write_u32(*upload_speed);
                w.write_u32(*queue_size);
                w.write_u32(*unknown);
                if !private_results.is_empty() {
                    write_file_list(&mut w, private_results);
                }
                return compress(&w.into_bytes());
            }
            Self::UserInfoResponse {
                description,
                picture,
                total_uploads,
                queue_size,
                slots_available,
                upload_allowed,
            } => {
                w.write_string(description);
                match picture {
                    Some(pic) => {
                        w.write_bool(true);
                        w.write_bytes(pic);
                    }
                    None => w.write_bool(false),
                }
                w.write_u32(*total_uploads);
                w.write_u32(*queue_size);
                w.write_bool(*slots_available);
                w.write_u32(upload_allowed.unwrap());
            }
            Self::FolderContentsRequest { token, directory, legacy_client } => {
                w.write_u32(*token);
                if *legacy_client {
                    w.write_string_legacy(directory);
                } else {
                    w.write_string(directory);
                }
            }
            Self::FolderContentsResponse { token, directory, folders } => {
                w.write_u32(*token);
                w.write_string(directory);
                write_folder_list(&mut w, folders);
                return compress(&w.into_bytes());
            }
            Self::TransferRequest { direction, token, file, filesize } => {
                w.write_u32(direction.as_u32());
                w.write_u32(*token);
                w.write_string(file);
                if *direction == TransferDirection::Upload {
                    w.write_u64(filesize.unwrap());
                }
            }
            Self::TransferResponse { token, allowed, reason, filesize } => {
                w.write_u32(*token);
                w.write_bool(*allowed);
                if let Some(reason) = reason {
                    w.write_string(reason);
                }
                if let Some(filesize) = filesize {
                    w.write_u64(*filesize);
                }
            }
            Self::PlaceholdUpload { file } | Self::UploadFailed { file } => w.write_string(file),
            Self::QueueUpload { file, legacy_client }
            | Self::PlaceInQueueRequest { file, legacy_client } => {
                if *legacy_client {
                    w.write_string_legacy(file);
                } else {
                    w.write_string(file);
                }
            }
            Self::PlaceInQueueResponse { filename, place } => {
                w.write_string(filename);
                w.write_u32(*place);
            }
            Self::UploadDenied { file, reason } => {
                w.write_string(file);
                w.write_string(reason);
            }
        }
        w.into_bytes()
    }

    pub fn parse(code: u32, payload: &[u8]) -> Result<Self, ProtocolError> {
        Ok(match code {
            4 => Self::SharedFileListRequest,
            5 => {
                let data = decompress(payload, MAX_SHARES_SIZE)?;
                let r = &mut MessageReader::new(&data);
                let shares = read_folder_list(r)?;
                let unknown = if r.has_remaining() { r.read_u32()? } else { 0 };
                let private_shares =
                    if r.has_remaining() { read_folder_list(r)? } else { Vec::new() };
                Self::SharedFileListResponse { shares, unknown, private_shares }
            }
            8 => {
                let r = &mut MessageReader::new(payload);
                Self::FileSearchRequest {
                    token: r.read_u32()?,
                    search_term: r.read_string()?,
                }
            }
            9 => {
                let data = decompress(payload, MAX_RESULTS_SIZE)?;
                let r = &mut MessageReader::new(&data);
                let username = r.read_string()?;
                let token = r.read_u32()?;
                let results = read_file_list(r, true)?;
                let free_upload_slots = r.read_bool()?;
                let upload_speed = r.read_u32()?;
                let queue_size = r.read_u32()?;
                let unknown = if r.has_remaining() { r.read_u32()? } else { 0 };
                let private_results =
                    if r.has_remaining() { read_file_list(r, true)? } else { Vec::new() };
                Self::FileSearchResponse {
                    username,
                    token,
                    results,
                    free_upload_slots,
                    upload_speed,
                    queue_size,
                    unknown,
                    private_results,
                }
            }
            15 => Self::UserInfoRequest,
            16 => {
                let r = &mut MessageReader::new(payload);
                let description = r.read_string()?;
                let has_picture = r.read_bool()?;
                let picture = if has_picture { Some(r.read_bytes()?) } else { None };
                let total_uploads = r.read_u32()?;
                let queue_size = r.read_u32()?;
                let slots_available = r.read_bool()?;
                let upload_allowed = if r.remaining() >= 4 { Some(r.read_u32()?) } else { None };
                Self::UserInfoResponse {
                    description,
                    picture,
                    total_uploads,
                    queue_size,
                    slots_available,
                    upload_allowed,
                }
            }
            36 => {
                let r = &mut MessageReader::new(payload);
                Self::FolderContentsRequest {
                    token: r.read_u32()?,
                    directory: r.read_string()?,
                    legacy_client: false,
                }
            }
            37 => {
                let data = decompress(payload, MAX_RESULTS_SIZE)?;
                let r = &mut MessageReader::new(&data);
                Self::FolderContentsResponse {
                    token: r.read_u32()?,
                    directory: r.read_string()?,
                    folders: read_folder_list(r)?,
                }
            }
            40 => {
                let r = &mut MessageReader::new(payload);
                let direction_value = r.read_u32()?;
                let direction = TransferDirection::from_u32(direction_value).ok_or(
                    ProtocolError::InvalidValue {
                        field: "direction",
                        value: direction_value as u64,
                    },
                )?;
                let token = r.read_u32()?;
                let file = r.read_string()?;
                let filesize = if direction == TransferDirection::Upload {
                    Some(r.read_u64()?)
                } else {
                    None
                };
                Self::TransferRequest { direction, token, file, filesize }
            }
            41 => {
                let r = &mut MessageReader::new(payload);
                let token = r.read_u32()?;
                let allowed = r.read_bool()?;
                let mut reason = None;
                let mut filesize = None;
                if r.has_remaining() {
                    if allowed {
                        filesize = Some(r.read_u64()?);
                    } else {
                        reason = Some(r.read_string()?);
                    }
                }
                Self::TransferResponse { token, allowed, reason, filesize }
            }
            42 => {
                let r = &mut MessageReader::new(payload);
                Self::PlaceholdUpload { file: r.read_string()? }
            }
            43 => {
                let r = &mut MessageReader::new(payload);
                Self::QueueUpload { file: r.read_string()?, legacy_client: false }
            }
            44 => {
                let r = &mut MessageReader::new(payload);
                Self::PlaceInQueueResponse {
                    filename: r.read_string()?,
                    place: r.read_u32()?,
                }
            }
            46 => {
                let r = &mut MessageReader::new(payload);
                Self::UploadFailed { file: r.read_string()? }
            }
            50 => {
                let r = &mut MessageReader::new(payload);
                Self::UploadDenied { file: r.read_string()?, reason: r.read_string()? }
            }
            51 => {
                let r = &mut MessageReader::new(payload);
                Self::PlaceInQueueRequest { file: r.read_string()?, legacy_client: false }
            }
            52 => Self::UploadQueueNotification,
            12547 => Self::UnknownPeerMessage,
            _ => return Err(ProtocolError::UnknownMessageCode { family: "peer", code }),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let payload = self.make_payload();
        let mut framed = MessageWriter::new();
        framed.write_u32(payload.len() as u32 + 4);
        framed.write_u32(self.code());
        framed.write_raw(&payload);
        framed.into_bytes()
    }
}
