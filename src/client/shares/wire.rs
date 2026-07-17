use crate::types::FileAttributes;

use super::ShareCatalog;

pub fn encode_shared_file_list(catalog: &ShareCatalog, is_buddy: bool) -> Vec<u8> {
    use std::io::Write;

    let mut framed = vec![0; 8];
    framed[4..8].copy_from_slice(&5u32.to_le_bytes());
    let mut encoder = flate2::write::ZlibEncoder::new(framed, flate2::Compression::new(4));
    write_u32_to(
        &mut encoder,
        catalog
            .folders_by_path
            .iter()
            .map(|&folder| &catalog.folders[folder as usize])
            .filter(|folder| is_buddy || !folder.buddy_only)
            .count() as u32,
    );
    for &folder_id in &catalog.folders_by_path {
        let folder = &catalog.folders[folder_id as usize];
        if !is_buddy && folder.buddy_only {
            continue;
        }
        write_string_to(&mut encoder, &folder.virtual_path);
        write_u32_to(&mut encoder, folder.files.len() as u32);
        for file_id in folder.files.clone() {
            let file = &catalog.files[file_id as usize];
            encoder.write_all(&[1]).unwrap();
            write_string_to(&mut encoder, &file.name);
            encoder.write_all(&file.size.to_le_bytes()).unwrap();
            write_u32_to(&mut encoder, 0);
            write_attributes_to(&mut encoder, &file.attributes);
        }
    }
    write_u32_to(&mut encoder, 0);
    let mut framed = encoder.finish().unwrap();
    let size = framed.len() as u32 - 4;
    framed[..4].copy_from_slice(&size.to_le_bytes());
    framed
}

fn write_u32_to(writer: &mut impl std::io::Write, value: u32) {
    writer.write_all(&value.to_le_bytes()).unwrap();
}

fn write_string_to(writer: &mut impl std::io::Write, value: &str) {
    write_u32_to(writer, value.len() as u32);
    writer.write_all(value.as_bytes()).unwrap();
}

fn write_attributes_to(writer: &mut impl std::io::Write, attributes: &FileAttributes) {
    let values = [
        (0u32, attributes.bitrate),
        (1, attributes.length),
        (2, attributes.vbr),
        (4, attributes.sample_rate),
        (5, attributes.bit_depth),
    ];
    write_u32_to(
        writer,
        values.iter().filter(|(_, value)| value.is_some()).count() as u32,
    );
    for (kind, value) in values {
        if let Some(value) = value {
            write_u32_to(writer, kind);
            write_u32_to(writer, value);
        }
    }
}
