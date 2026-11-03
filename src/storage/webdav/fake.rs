use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use reqwest::Url;

use super::client::WebDav;
use super::path::parent;

const PREFIX: &str = "/dav";
const CREDENTIALS: &str = "Basic dXNlcjpwYXNz";
const HREF: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b',')
    .remove(b':')
    .remove(b';')
    .remove(b'=')
    .remove(b'@');

enum Node {
    Collection,
    File(Vec<u8>),
}

#[derive(Default)]
struct Server {
    nodes: BTreeMap<String, Node>,
    requests: Vec<String>,
    aborted_puts: Vec<String>,
    refuse_empty_puts: bool,
    reject_puts: Option<StatusCode>,
}

#[derive(Clone)]
pub(super) struct Fake {
    pub(super) url: String,
    server: Arc<Mutex<Server>>,
}

impl Fake {
    pub(super) async fn start() -> Fake {
        let server = Arc::new(Mutex::new(Server::default()));
        server
            .lock()
            .unwrap()
            .nodes
            .insert("/".to_owned(), Node::Collection);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}{PREFIX}/", listener.local_addr().unwrap());
        let app = Router::new().fallback(handle).with_state(server.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Fake { url, server }
    }

    pub(super) fn client(&self) -> WebDav {
        WebDav::new(&self.url, "user", "pass").unwrap()
    }

    pub(super) fn collection(&self, path: &str) {
        let mut server = self.server.lock().unwrap();
        server.ancestors(path);
        server.nodes.insert(path.to_owned(), Node::Collection);
    }

    pub(super) fn file(&self, path: &str, bytes: &[u8]) {
        let mut server = self.server.lock().unwrap();
        server.ancestors(path);
        server
            .nodes
            .insert(path.to_owned(), Node::File(bytes.to_vec()));
    }

    pub(super) fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self.server.lock().unwrap().nodes.get(path) {
            Some(Node::File(bytes)) => Some(bytes.clone()),
            _ => None,
        }
    }

    pub(super) fn exists(&self, path: &str) -> bool {
        self.server.lock().unwrap().nodes.contains_key(path)
    }

    pub(super) fn requests(&self) -> Vec<String> {
        self.server.lock().unwrap().requests.clone()
    }

    pub(super) fn refuse_empty_puts(&self) {
        self.server.lock().unwrap().refuse_empty_puts = true;
    }

    pub(super) fn reject_puts(&self, status: StatusCode) {
        self.server.lock().unwrap().reject_puts = Some(status);
    }

    async fn until(&self, done: impl Fn(&Server) -> bool) {
        for _ in 0..1000 {
            if done(&self.server.lock().unwrap()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the fake WebDAV server never reached the expected state");
    }

    pub(super) async fn received(&self, request: &str, count: usize) {
        self.until(|server| {
            server
                .requests
                .iter()
                .filter(|seen| *seen == request)
                .count()
                == count
        })
        .await;
    }

    pub(super) async fn aborted(&self, path: &str, count: usize) {
        self.until(|server| {
            server
                .aborted_puts
                .iter()
                .filter(|aborted| *aborted == path)
                .count()
                == count
        })
        .await;
    }
}

fn local_path(uri_path: &str) -> String {
    let segments: Vec<String> = uri_path
        .strip_prefix(PREFIX)
        .expect("requests stay below the prefix")
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            percent_decode_str(segment)
                .decode_utf8()
                .unwrap()
                .into_owned()
        })
        .collect();
    format!("/{}", segments.join("/"))
}

fn text(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    Some(headers.get(name)?.to_str().unwrap().to_owned())
}

async fn handle(State(server): State<Arc<Mutex<Server>>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    if parts
        .headers
        .get(header::AUTHORIZATION)
        .is_none_or(|value| value != CREDENTIALS)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let path = local_path(parts.uri.path());
    server
        .lock()
        .unwrap()
        .requests
        .push(format!("{} {path}", parts.method));
    if parts.method == Method::PUT {
        return put(&server, path, body).await;
    }
    let headers = &parts.headers;
    let mut server = server.lock().unwrap();
    match parts.method.as_str() {
        "PROPFIND" => {
            let depth = text(headers, header::HeaderName::from_static("depth"));
            server.propfind(
                &path,
                depth.as_deref(),
                &text(headers, header::HOST).unwrap(),
            )
        }
        "GET" => server.get(&path, text(headers, header::RANGE)),
        "MKCOL" => server.mkcol(&path),
        "MOVE" => server.move_file(
            &path,
            &text(headers, header::HeaderName::from_static("destination")).unwrap(),
            text(headers, header::HeaderName::from_static("overwrite")).as_deref(),
        ),
        "DELETE" => server.delete(&path),
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

async fn put(server: &Arc<Mutex<Server>>, path: String, body: Body) -> Response {
    let refusal = {
        let server = server.lock().unwrap();
        if matches!(server.nodes.get(&path), Some(Node::Collection)) {
            Some(StatusCode::METHOD_NOT_ALLOWED)
        } else if !server.is_collection(parent(&path)) {
            Some(StatusCode::CONFLICT)
        } else {
            server.reject_puts
        }
    };
    if let Some(status) = refusal {
        tokio::spawn(async move {
            let _unread = body;
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        return (status, status.canonical_reason().unwrap()).into_response();
    }
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        server.lock().unwrap().aborted_puts.push(path);
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mut server = server.lock().unwrap();
    if bytes.is_empty() && server.refuse_empty_puts {
        return (StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed").into_response();
    }
    server.nodes.insert(path, Node::File(bytes.to_vec()));
    StatusCode::CREATED.into_response()
}

fn describe(xml: &mut String, path: &str, node: &Node, host: &str) {
    let encoded: String = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| format!("/{}", utf8_percent_encode(segment, HREF)))
        .collect();
    let encoded = encoded.replace('&', "&amp;");
    match node {
        Node::Collection => xml.push_str(&format!(
            "<d:response><d:href>{PREFIX}{encoded}/</d:href><d:propstat><d:prop><d:resourcetype>\
             <d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status>\
             </d:propstat><d:propstat><d:prop><d:getcontentlength/></d:prop>\
             <d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>"
        )),
        Node::File(bytes) => xml.push_str(&format!(
            "<d:response><d:href>http://{host}{PREFIX}{encoded}</d:href><d:propstat><d:prop>\
             <d:resourcetype/><d:getcontentlength>{}</d:getcontentlength></d:prop>\
             <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
            bytes.len()
        )),
    }
}

impl Server {
    fn ancestors(&mut self, path: &str) {
        let mut current = parent(path);
        while current != "/" {
            self.nodes.insert(current.to_owned(), Node::Collection);
            current = parent(current);
        }
    }

    fn is_collection(&self, path: &str) -> bool {
        matches!(self.nodes.get(path), Some(Node::Collection))
    }

    fn propfind(&self, path: &str, depth: Option<&str>, host: &str) -> Response {
        let Some(node) = self.nodes.get(path) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let listed = match depth {
            Some("0") => false,
            Some("1") => matches!(node, Node::Collection),
            _ => return (StatusCode::FORBIDDEN, "Depth infinity is refused").into_response(),
        };
        let mut xml =
            String::from(r#"<?xml version="1.0" encoding="UTF-8"?><d:multistatus xmlns:d="DAV:">"#);
        describe(&mut xml, path, node, host);
        if listed {
            let prefix = if path == "/" {
                "/".to_owned()
            } else {
                format!("{path}/")
            };
            for (child, node) in &self.nodes {
                if child
                    .strip_prefix(&prefix)
                    .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
                {
                    describe(&mut xml, child, node, host);
                }
            }
        }
        xml.push_str("</d:multistatus>");
        (
            StatusCode::MULTI_STATUS,
            [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
            xml,
        )
            .into_response()
    }

    fn get(&self, path: &str, range: Option<String>) -> Response {
        let Some(Node::File(bytes)) = self.nodes.get(path) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some(range) = range else {
            return bytes.clone().into_response();
        };
        let start: usize = range
            .strip_prefix("bytes=")
            .and_then(|range| range.strip_suffix('-'))
            .unwrap()
            .parse()
            .unwrap();
        let length = bytes.len();
        if start >= length {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{length}"))],
            )
                .into_response();
        }
        (
            StatusCode::PARTIAL_CONTENT,
            [(
                header::CONTENT_RANGE,
                format!("bytes {start}-{}/{length}", length - 1),
            )],
            bytes[start..].to_vec(),
        )
            .into_response()
    }

    fn mkcol(&mut self, path: &str) -> Response {
        if self.nodes.contains_key(path) {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        if !self.is_collection(parent(path)) {
            return StatusCode::CONFLICT.into_response();
        }
        self.nodes.insert(path.to_owned(), Node::Collection);
        StatusCode::CREATED.into_response()
    }

    fn move_file(&mut self, path: &str, destination: &str, overwrite: Option<&str>) -> Response {
        let destination = local_path(Url::parse(destination).unwrap().path());
        if !matches!(self.nodes.get(path), Some(Node::File(_))) {
            return StatusCode::NOT_FOUND.into_response();
        }
        let exists = self.nodes.contains_key(&destination);
        if exists && overwrite == Some("F") {
            return StatusCode::PRECONDITION_FAILED.into_response();
        }
        if !self.is_collection(parent(&destination)) {
            return StatusCode::CONFLICT.into_response();
        }
        let node = self.nodes.remove(path).unwrap();
        self.nodes.insert(destination, node);
        if exists {
            StatusCode::NO_CONTENT.into_response()
        } else {
            StatusCode::CREATED.into_response()
        }
    }

    fn delete(&mut self, path: &str) -> Response {
        if self.nodes.remove(path).is_none() {
            return StatusCode::NOT_FOUND.into_response();
        }
        let prefix = format!("{path}/");
        self.nodes
            .retain(|candidate, _| !candidate.starts_with(&prefix));
        StatusCode::NO_CONTENT.into_response()
    }
}
