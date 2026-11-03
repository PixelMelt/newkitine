use std::fmt::Display;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use futures_util::TryStreamExt;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_encode};
use reqwest::header::CONTENT_TYPE;
use reqwest::redirect::Policy;
use reqwest::{Method, RequestBuilder, Response, StatusCode, Url};
use tokio_util::io::StreamReader;

use super::multistatus::{self, Entry};
use super::path::{collection, decode_segments};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const ERROR_BODY_BYTES: usize = 256;
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
const PROPFIND_BODY: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
    <D:propfind xmlns:D=\"DAV:\"><D:prop><D:resourcetype/><D:getcontentlength/></D:prop>\
    </D:propfind>";

#[derive(Clone)]
pub struct WebDav {
    http: reqwest::Client,
    endpoint: Arc<Endpoint>,
}

struct Endpoint {
    base: Url,
    username: String,
    password: String,
}

pub(super) struct Answer {
    context: String,
    pub(super) response: Response,
}

pub(super) fn method(name: &'static str) -> Method {
    Method::from_bytes(name.as_bytes()).expect("WebDAV method names are HTTP tokens")
}

impl WebDav {
    pub fn new(url: &str, username: &str, password: &str) -> Result<WebDav, String> {
        let base = Url::parse(url).map_err(|error| format!("WebDAV URL is invalid: {error}"))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(format!(
                "WebDAV URL is invalid: it must be http or https, not {}",
                base.scheme()
            ));
        }
        if !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(
                "WebDAV URL is invalid: it cannot carry credentials, a query or a fragment"
                    .to_owned(),
            );
        }
        decode_segments(base.path())
            .map_err(|error| format!("WebDAV URL {url:?} is invalid: {error}"))?;
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(Policy::none())
            .build()
            .map_err(|error| format!("cannot build the WebDAV client: {error}"))?;
        Ok(WebDav {
            http,
            endpoint: Arc::new(Endpoint {
                base,
                username: username.to_owned(),
                password: password.to_owned(),
            }),
        })
    }

    fn url(&self, path: &[u8]) -> Url {
        let mut encoded = self.endpoint.base.path().trim_end_matches('/').to_owned();
        for segment in path.split(|byte| *byte == b'/').skip(1) {
            encoded.push('/');
            encoded.extend(percent_encode(segment, SEGMENT));
        }
        let mut url = self.endpoint.base.clone();
        url.set_path(&encoded);
        url
    }

    pub(super) fn absolute(&self, path: &[u8]) -> String {
        self.url(path).into()
    }

    pub(super) async fn call(
        &self,
        method: Method,
        path: &[u8],
        build: impl FnOnce(RequestBuilder) -> RequestBuilder,
    ) -> io::Result<Answer> {
        self.stream(method, path, |request| {
            build(request.timeout(REQUEST_TIMEOUT))
        })
        .await
    }

    pub(super) async fn stream(
        &self,
        method: Method,
        path: &[u8],
        build: impl FnOnce(RequestBuilder) -> RequestBuilder,
    ) -> io::Result<Answer> {
        let context = format!("{method} {}", String::from_utf8_lossy(path));
        let request = self
            .http
            .request(method, self.url(path))
            .basic_auth(&self.endpoint.username, Some(&self.endpoint.password));
        match build(request).send().await {
            Ok(response) => Ok(Answer { context, response }),
            Err(error) => Err(io::Error::other(format!(
                "{context} failed: {}",
                describe(error)
            ))),
        }
    }

    async fn propfind(&self, path: &[u8], depth: &'static str) -> io::Result<Vec<Entry>> {
        let answer = self
            .call(method("PROPFIND"), path, |request| {
                request
                    .header("Depth", depth)
                    .header(CONTENT_TYPE, "application/xml; charset=utf-8")
                    .body(PROPFIND_BODY)
            })
            .await?;
        if answer.status() != StatusCode::MULTI_STATUS {
            return Err(answer.refused().await);
        }
        let summary = answer.summary();
        let body = StreamReader::new(answer.response.bytes_stream().map_err(io::Error::other));
        multistatus::parse(body, &self.endpoint.base)
            .await
            .map_err(|error| {
                io::Error::other(format!("{summary} with an unreadable body: {error}"))
            })
    }

    pub(super) async fn list(&self, path: &[u8]) -> io::Result<Vec<Entry>> {
        let mut described = false;
        let mut children = Vec::new();
        for entry in self.propfind(&collection(path), "1").await? {
            if entry.path == path {
                if !entry.collection {
                    return Err(io::Error::other(format!(
                        "PROPFIND {} described a file, not a collection",
                        String::from_utf8_lossy(path)
                    )));
                }
                described = true;
                continue;
            }
            if entry.parent() != path {
                return Err(io::Error::other(format!(
                    "PROPFIND {} listed {} below another collection",
                    String::from_utf8_lossy(path),
                    String::from_utf8_lossy(&entry.path)
                )));
            }
            children.push(entry);
        }
        if !described {
            return Err(io::Error::other(format!(
                "PROPFIND {} did not describe it",
                String::from_utf8_lossy(path)
            )));
        }
        Ok(children)
    }

    pub(super) async fn stat(&self, path: &[u8]) -> io::Result<Entry> {
        self.propfind(path, "0")
            .await?
            .into_iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| {
                io::Error::other(format!(
                    "PROPFIND {} did not describe it",
                    String::from_utf8_lossy(path)
                ))
            })
    }
}

impl Answer {
    pub(super) fn status(&self) -> StatusCode {
        self.response.status()
    }

    fn summary(&self) -> String {
        format!("{} answered {}", self.context, self.status())
    }

    pub(super) fn unusable(&self, problem: impl Display) -> io::Error {
        io::Error::other(format!("{} {problem}", self.summary()))
    }

    pub(super) async fn refused(mut self) -> io::Error {
        let status = self.response.status();
        let mut body = Vec::new();
        while body.len() < ERROR_BODY_BYTES
            && let Ok(Some(chunk)) = self.response.chunk().await
        {
            body.extend_from_slice(&chunk[..chunk.len().min(ERROR_BODY_BYTES - body.len())]);
        }
        let kind = if status == StatusCode::NOT_FOUND {
            io::ErrorKind::NotFound
        } else {
            io::ErrorKind::Other
        };
        match String::from_utf8_lossy(&body).trim() {
            "" => io::Error::new(kind, self.summary()),
            body => io::Error::new(kind, format!("{}: {body}", self.summary())),
        }
    }
}

fn describe(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut message = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        message = format!("{message}: {cause}");
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_must_be_plain_http_or_https() {
        assert!(WebDav::new("https://dav.example/remote.php/dav/", "u", "p").is_ok());
        assert!(WebDav::new("http://127.0.0.1:8080", "u", "p").is_ok());
        for url in [
            "ftp://dav.example/",
            "dav.example/dav",
            "http://user:secret@dav.example/",
            "https://dav.example/dav?x=1",
            "https://dav.example/dav#top",
            "https://dav.example/a%2Fb/",
        ] {
            assert!(
                WebDav::new(url, "u", "p").is_err(),
                "{url} must be rejected"
            );
        }
        for url in [
            "https://user:secret@dav.example:invalid/",
            "ftp://user:secret@dav.example/",
            "http://user:secret@dav.example/",
        ] {
            let error = WebDav::new(url, "u", "p").err().unwrap();
            assert!(!error.contains("secret"), "{error}");
        }
    }

    #[test]
    fn urls_encode_each_segment_onto_the_base_path() {
        let client = WebDav::new("https://dav.example/remote.php/dav/", "u", "p").unwrap();
        assert_eq!(
            client.absolute("/A b/片 #%?.mp3".as_bytes()),
            "https://dav.example/remote.php/dav/A%20b/%E7%89%87%20%23%25%3F.mp3"
        );
        assert_eq!(client.absolute(b"/"), "https://dav.example/remote.php/dav/");
        assert_eq!(
            client.absolute(b"/Music/"),
            "https://dav.example/remote.php/dav/Music/"
        );
        let root = WebDav::new("http://127.0.0.1:8080", "u", "p").unwrap();
        assert_eq!(root.absolute(b"/a\\b"), "http://127.0.0.1:8080/a%5Cb");
    }
}
