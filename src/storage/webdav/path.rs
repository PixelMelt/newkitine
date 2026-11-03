use percent_encoding::percent_decode_str;

pub(super) fn remote_path(path: &str) -> Result<String, String> {
    let invalid = || {
        format!(
            "remote path {path:?} is invalid: it must start with / and have no empty, . or .. \
             segments"
        )
    };
    let rest = path.strip_prefix('/').ok_or_else(invalid)?;
    if rest.is_empty() {
        return Ok("/".to_owned());
    }
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest
        .split('/')
        .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Err(invalid());
    }
    Ok(format!("/{rest}"))
}

pub(super) fn decode_segments(encoded: &str) -> Result<Vec<Vec<u8>>, String> {
    let rest = encoded
        .strip_prefix('/')
        .ok_or_else(|| format!("path {encoded:?} is not absolute"))?;
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    rest.strip_suffix('/')
        .unwrap_or(rest)
        .split('/')
        .map(|segment| {
            let decoded: Vec<u8> = percent_decode_str(segment).collect();
            if decoded.is_empty() || decoded == b"." || decoded == b".." || decoded.contains(&b'/')
            {
                return Err(format!(
                    "path {encoded:?} has an empty, dot or encoded slash segment"
                ));
            }
            Ok(decoded)
        })
        .collect()
}

pub(super) fn child(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

pub(super) fn parent(path: &str) -> &str {
    let (parent, _) = path
        .rsplit_once('/')
        .expect("remote paths start with a slash");
    if parent.is_empty() { "/" } else { parent }
}

pub(super) fn collection(path: &[u8]) -> Vec<u8> {
    if path == b"/" {
        return path.to_vec();
    }
    [path, b"/"].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths_are_absolute_without_empty_or_dot_segments() {
        assert_eq!(remote_path("/"), Ok("/".to_owned()));
        assert_eq!(remote_path("/Music/"), Ok("/Music".to_owned()));
        assert_eq!(remote_path("/Music/Album"), Ok("/Music/Album".to_owned()));
        for invalid in [
            "", "Music", "//", "/Music//", "/a//b", "/./a", "/a/..", "/a/.",
        ] {
            assert!(
                remote_path(invalid).is_err(),
                "{invalid:?} must be rejected"
            );
        }
    }

    #[test]
    fn encoded_paths_decode_segment_by_segment() {
        assert_eq!(decode_segments("/"), Ok(Vec::new()));
        assert_eq!(
            decode_segments("/dav/A%20b/%E7%89%87/"),
            Ok(vec![
                b"dav".to_vec(),
                b"A b".to_vec(),
                "片".as_bytes().to_vec()
            ])
        );
        for invalid in ["dav", "/a//b", "/a/%2Fb", "/a/%2e%2E", "/a/."] {
            assert!(
                decode_segments(invalid).is_err(),
                "{invalid:?} must be rejected"
            );
        }
    }

    #[test]
    fn children_and_parents_handle_the_root() {
        assert_eq!(child("/", "a"), "/a");
        assert_eq!(child("/a", "b"), "/a/b");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("/a/b"), "/a");
        assert_eq!(collection(b"/"), b"/");
        assert_eq!(collection(b"/a"), b"/a/");
    }
}
