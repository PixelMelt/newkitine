use std::io::{self, Write};

use super::ShareCatalog;

pub fn encode_shared_file_list(catalog: &ShareCatalog, is_buddy: bool) -> Vec<u8> {
    let mut framed = vec![0; 8];
    framed[4..8].copy_from_slice(&5u32.to_le_bytes());
    let mut encoder = io::BufWriter::with_capacity(
        1 << 16,
        flate2::write::ZlibEncoder::new(framed, flate2::Compression::new(4)),
    );
    write_folders(&mut encoder, catalog, is_buddy).expect("in-memory share list encoding");
    let mut framed = encoder
        .into_inner()
        .expect("in-memory share list flush")
        .finish()
        .expect("in-memory share list compression");
    let size = framed.len() as u32 - 4;
    framed[..4].copy_from_slice(&size.to_le_bytes());
    framed
}

fn write_folders(
    writer: &mut impl Write,
    catalog: &ShareCatalog,
    is_buddy: bool,
) -> io::Result<()> {
    write_u32_to(
        writer,
        catalog
            .folders_by_path
            .iter()
            .map(|&folder| &catalog.folders[folder as usize])
            .filter(|folder| is_buddy || !folder.buddy_only)
            .count() as u32,
    )?;
    for &folder_id in &catalog.folders_by_path {
        let folder = &catalog.folders[folder_id as usize];
        if !is_buddy && folder.buddy_only {
            continue;
        }
        write_string_to(writer, &folder.virtual_path)?;
        write_u32_to(writer, folder.files.len() as u32)?;
        for file in catalog.folder_files(folder) {
            writer.write_all(&[1])?;
            write_string_to(writer, &file.name)?;
            writer.write_all(&file.size.to_le_bytes())?;
            write_u32_to(writer, 0)?;
            let pairs = file.attributes.wire_pairs();
            write_u32_to(writer, pairs.iter().flatten().count() as u32)?;
            for (kind, value) in pairs.into_iter().flatten() {
                write_u32_to(writer, kind)?;
                write_u32_to(writer, value)?;
            }
        }
    }
    write_u32_to(writer, 0)
}

pub(super) fn write_u32_to(writer: &mut impl Write, value: u32) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

pub(super) fn write_string_to(writer: &mut impl Write, value: &str) -> io::Result<()> {
    write_u32_to(writer, value.len() as u32)?;
    writer.write_all(value.as_bytes())
}
