use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use reqwest::Url;
use tokio::io::{AsyncBufRead, AsyncRead, ReadBuf};

use super::path::decode_segments;

const DAV: &str = "DAV:";
const TOKEN_LIMIT: usize = 64 * 1024;
const DEPTH_LIMIT: usize = 32;

#[derive(Debug, PartialEq)]
pub(super) struct Entry {
    pub(super) path: Vec<u8>,
    pub(super) collection: bool,
    pub(super) size: Option<u64>,
}

impl Entry {
    fn slash(&self) -> usize {
        self.path
            .iter()
            .rposition(|byte| *byte == b'/')
            .expect("remote paths start with a slash")
    }

    pub(super) fn name(&self) -> &[u8] {
        &self.path[self.slash() + 1..]
    }

    pub(super) fn parent(&self) -> &[u8] {
        match self.slash() {
            0 => b"/",
            slash => &self.path[..slash],
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tag {
    Multistatus,
    Response,
    Href,
    Propstat,
    Prop,
    ResourceType,
    Collection,
    ContentLength,
    Status,
    Other,
}

impl Tag {
    fn of(namespace: &ResolveResult, local: &str) -> Tag {
        if !matches!(namespace, ResolveResult::Bound(Namespace(DAV))) {
            return Tag::Other;
        }
        match local {
            "multistatus" => Tag::Multistatus,
            "response" => Tag::Response,
            "href" => Tag::Href,
            "propstat" => Tag::Propstat,
            "prop" => Tag::Prop,
            "resourcetype" => Tag::ResourceType,
            "collection" => Tag::Collection,
            "getcontentlength" => Tag::ContentLength,
            "status" => Tag::Status,
            _ => Tag::Other,
        }
    }
}

const IN_MULTISTATUS: &[Tag] = &[Tag::Multistatus];
const IN_RESPONSE: &[Tag] = &[Tag::Multistatus, Tag::Response];
const IN_PROPSTAT: &[Tag] = &[Tag::Multistatus, Tag::Response, Tag::Propstat];
const IN_PROP: &[Tag] = &[Tag::Multistatus, Tag::Response, Tag::Propstat, Tag::Prop];
const IN_RESOURCETYPE: &[Tag] = &[
    Tag::Multistatus,
    Tag::Response,
    Tag::Propstat,
    Tag::Prop,
    Tag::ResourceType,
];

#[derive(Clone, Copy)]
enum Field {
    Href,
    ResponseStatus,
    PropstatStatus,
    ContentLength,
}

#[derive(Default)]
struct Response {
    href: Option<String>,
    status: Option<String>,
    collection: bool,
    size: Option<String>,
}

#[derive(Default)]
struct Propstat {
    status: Option<String>,
    collection: bool,
    size: Option<String>,
}

struct Parser<'a> {
    base: &'a Url,
    base_segments: Vec<Vec<u8>>,
    stack: Vec<Tag>,
    root: bool,
    response: Option<Response>,
    propstat: Option<Propstat>,
    field: Option<(Field, String)>,
    entries: Vec<Entry>,
}

pub(super) async fn parse(
    reader: impl AsyncBufRead + Unpin,
    base: &Url,
) -> Result<Vec<Entry>, String> {
    let mut parser = Parser {
        base,
        base_segments: decode_segments(base.path())?,
        stack: Vec::new(),
        root: false,
        response: None,
        propstat: None,
        field: None,
        entries: Vec::new(),
    };
    let mut reader = NsReader::from_reader(Bounded {
        inner: reader,
        consumed: 0,
    });
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        reader.get_mut().consumed = 0;
        let (namespace, event) = reader
            .read_resolved_event_into_async(&mut buffer)
            .await
            .map_err(|error| error.to_string())?;
        match event {
            Event::Start(_) if parser.stack.len() == DEPTH_LIMIT => {
                return Err(format!("the body nests elements deeper than {DEPTH_LIMIT}"));
            }
            Event::Start(start) => {
                let tag = Tag::of(&namespace, start.local_name().as_ref());
                parser.open(tag)?;
                parser.stack.push(tag);
            }
            Event::Empty(start) => {
                let tag = Tag::of(&namespace, start.local_name().as_ref());
                parser.open(tag)?;
                parser.close(tag)?;
            }
            Event::End(_) => {
                let tag = parser
                    .stack
                    .pop()
                    .expect("the reader checks that end tags match");
                parser.close(tag)?;
            }
            Event::Text(text) => parser.text(&text.xml10_content())?,
            Event::CData(data) => parser.text(&data.xml10_content())?,
            Event::GeneralRef(reference) => parser.text(&resolve(&reference)?)?,
            Event::Eof if parser.stack.is_empty() => break,
            Event::Eof => return Err("the body ends inside an open element".to_owned()),
            _ => {}
        }
    }
    if !parser.root {
        return Err("the body is not a DAV:multistatus".to_owned());
    }
    Ok(parser.entries)
}

struct Bounded<R> {
    inner: R,
    consumed: usize,
}

impl<R: AsyncBufRead + Unpin> AsyncRead for Bounded<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let available = ready!(self.as_mut().poll_fill_buf(cx))?;
        let count = available.len().min(buf.remaining());
        buf.put_slice(&available[..count]);
        self.consume(count);
        Poll::Ready(Ok(()))
    }
}

impl<R: AsyncBufRead + Unpin> AsyncBufRead for Bounded<R> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        if this.consumed > TOKEN_LIMIT {
            return Poll::Ready(Err(io::Error::other(format!(
                "an XML token exceeds {TOKEN_LIMIT} bytes"
            ))));
        }
        let budget = TOKEN_LIMIT + 1 - this.consumed;
        let available = ready!(Pin::new(&mut this.inner).poll_fill_buf(cx))?;
        Poll::Ready(Ok(&available[..available.len().min(budget)]))
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        let this = self.get_mut();
        this.consumed += amount;
        Pin::new(&mut this.inner).consume(amount);
    }
}

fn resolve(reference: &BytesRef) -> Result<String, String> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|error| error.to_string())?
    {
        return Ok(character.to_string());
    }
    resolve_predefined_entity(reference)
        .map(str::to_owned)
        .ok_or_else(|| format!("unknown entity &{};", reference.as_ref()))
}

impl Parser<'_> {
    fn open(&mut self, tag: Tag) -> Result<(), String> {
        let within = self.stack.as_slice();
        match tag {
            Tag::Multistatus if within.is_empty() && !self.root => self.root = true,
            _ if within.is_empty() => return Err("the body is not a DAV:multistatus".to_owned()),
            Tag::Response if within == IN_MULTISTATUS => self.response = Some(Response::default()),
            Tag::Href if within == IN_RESPONSE => self.field = Some((Field::Href, String::new())),
            Tag::Status if within == IN_RESPONSE => {
                self.field = Some((Field::ResponseStatus, String::new()))
            }
            Tag::Propstat if within == IN_RESPONSE => self.propstat = Some(Propstat::default()),
            Tag::Status if within == IN_PROPSTAT => {
                self.field = Some((Field::PropstatStatus, String::new()))
            }
            Tag::ContentLength if within == IN_PROP => {
                self.field = Some((Field::ContentLength, String::new()))
            }
            Tag::Collection if within == IN_RESOURCETYPE => self.propstat_mut().collection = true,
            _ => {}
        }
        Ok(())
    }

    fn close(&mut self, tag: Tag) -> Result<(), String> {
        let within = self.stack.as_slice();
        match tag {
            Tag::Href | Tag::Status | Tag::ContentLength => {
                if let Some((field, text)) = self.field.take() {
                    self.store(field, text.trim().to_owned())?;
                }
            }
            Tag::Propstat if within == IN_RESPONSE => {
                let propstat = self.propstat.take().expect("inside a propstat");
                let status = propstat
                    .status
                    .ok_or_else(|| "a propstat has no status".to_owned())?;
                if successful(&status)? {
                    let response = self.response_mut();
                    response.collection |= propstat.collection;
                    if propstat.size.is_some() {
                        response.size = propstat.size;
                    }
                }
            }
            Tag::Response if within == IN_MULTISTATUS => {
                let response = self.response.take().expect("inside a response");
                let entry = self.entry(response)?;
                self.entries.push(entry);
            }
            _ => {}
        }
        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<(), String> {
        if let Some((_, value)) = &mut self.field {
            if value.len() + text.len() > TOKEN_LIMIT {
                return Err(format!("a property value exceeds {TOKEN_LIMIT} bytes"));
            }
            value.push_str(text);
        }
        Ok(())
    }

    fn store(&mut self, field: Field, text: String) -> Result<(), String> {
        match field {
            Field::Href => {
                let response = self.response_mut();
                if response.href.is_some() {
                    return Err("a response has several hrefs".to_owned());
                }
                response.href = Some(text);
            }
            Field::ResponseStatus => self.response_mut().status = Some(text),
            Field::PropstatStatus => self.propstat_mut().status = Some(text),
            Field::ContentLength => self.propstat_mut().size = Some(text),
        }
        Ok(())
    }

    fn response_mut(&mut self) -> &mut Response {
        self.response.as_mut().expect("inside a response")
    }

    fn propstat_mut(&mut self) -> &mut Propstat {
        self.propstat.as_mut().expect("inside a propstat")
    }

    fn entry(&self, response: Response) -> Result<Entry, String> {
        let href = response
            .href
            .ok_or_else(|| "a response has no href".to_owned())?;
        if let Some(status) = &response.status
            && !successful(status)?
        {
            return Err(format!("{href} answered {status}"));
        }
        let path = self.remote_path(&href)?;
        let size = match response.size {
            Some(size) if !response.collection && !size.is_empty() => Some(
                size.parse::<u64>()
                    .map_err(|_| format!("{href} has an invalid getcontentlength {size:?}"))?,
            ),
            _ => None,
        };
        Ok(Entry {
            path,
            collection: response.collection,
            size,
        })
    }

    fn remote_path(&self, href: &str) -> Result<Vec<u8>, String> {
        let url = if href.starts_with('/') {
            self.base.join(href)
        } else {
            Url::parse(href)
        }
        .map_err(|error| format!("href {href:?} is not an absolute URL or path: {error}"))?;
        let segments = decode_segments(url.path()).map_err(|error| format!("href {error}"))?;
        let relative = segments
            .strip_prefix(self.base_segments.as_slice())
            .ok_or_else(|| format!("href {href:?} is outside {}", self.base.path()))?;
        let mut path = vec![b'/'];
        path.extend(relative.join(&b'/'));
        Ok(path)
    }
}

fn successful(status: &str) -> Result<bool, String> {
    let code = status
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("invalid status line {status:?}"))?;
    Ok((200..300).contains(&code))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://dav.example/remote.php/dav/").unwrap()
    }

    async fn entries(xml: &str) -> Result<Vec<Entry>, String> {
        parse(xml.as_bytes(), &base()).await
    }

    fn folder(path: &[u8]) -> Entry {
        Entry {
            path: path.to_vec(),
            collection: true,
            size: None,
        }
    }

    fn file(path: &[u8], size: u64) -> Entry {
        Entry {
            path: path.to_vec(),
            collection: false,
            size: Some(size),
        }
    }

    fn single(href: &str) -> String {
        format!(
            "<D:multistatus xmlns:D=\"DAV:\"><D:response><D:href>{href}</D:href><D:propstat>\
             <D:prop><D:getcontentlength>1</D:getcontentlength></D:prop>\
             <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"
        )
    }

    #[tokio::test]
    async fn every_prefix_and_the_default_namespace_resolve_to_dav() {
        let prefixed = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/remote.php/dav/Music/</D:href>
    <D:propstat>
      <D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
    <D:propstat>
      <D:prop><D:getcontentlength/></D:prop>
      <D:status>HTTP/1.1 404 Not Found</D:status>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/remote.php/dav/Music/song.mp3</D:href>
    <D:propstat>
      <D:prop><D:resourcetype/><D:getcontentlength>1234</D:getcontentlength></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#;
        let lower = prefixed.replace("D:", "d:").replace("xmlns:D=", "xmlns:d=");
        let default = prefixed.replace("D:", "").replace("xmlns:D=", "xmlns=");
        let foreign_prefix = prefixed.replace("D:", "lp1:").replace(
            "xmlns:D=\"DAV:\"",
            "xmlns:lp1=\"DAV:\" xmlns:D=\"urn:other\"",
        );
        for xml in [prefixed.to_owned(), lower, default, foreign_prefix] {
            assert_eq!(
                entries(&xml).await.unwrap(),
                [folder(b"/Music"), file(b"/Music/song.mp3", 1234)],
                "{xml}"
            );
        }
    }

    #[tokio::test]
    async fn elements_outside_dav_are_ignored() {
        let xml = r#"<multistatus xmlns="DAV:" xmlns:x="urn:other">
<response>
  <href>/remote.php/dav/a.mp3</href>
  <x:href>/remote.php/dav/elsewhere</x:href>
  <propstat>
    <prop>
      <x:getcontentlength>99</x:getcontentlength>
      <getcontentlength>5</getcontentlength>
      <x:resourcetype><collection/></x:resourcetype>
      <resourcetype><x:collection/></resourcetype>
    </prop>
    <status>HTTP/1.1 200 OK</status>
  </propstat>
</response>
</multistatus>"#;
        assert_eq!(entries(xml).await.unwrap(), [file(b"/a.mp3", 5)]);

        let not_dav = r#"<D:multistatus xmlns:D="urn:not-dav"></D:multistatus>"#;
        assert!(entries(not_dav).await.is_err());
        assert!(entries("<html><body>207</body></html>").await.is_err());
    }

    #[tokio::test]
    async fn hrefs_decode_from_absolute_urls_and_paths() {
        let xml = r#"<multistatus xmlns="DAV:">
<response>
  <href>https://dav.example/remote.php/dav/</href>
  <propstat><prop><resourcetype><collection/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat>
</response>
<response>
  <href>
    http://internal.host:8080/remote.php/dav/A%20b/
  </href>
  <propstat><prop><resourcetype><collection/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat>
</response>
<response>
  <href>/remote.php/dav/A%20b/%E7%89%87%20100%25%20%23%201.flac</href>
  <propstat><prop><getcontentlength> 7 </getcontentlength><resourcetype/></prop><status>HTTP/1.1 200 OK</status></propstat>
</response>
<response>
  <href>/remote.php/dav/A%20b/rock%20&amp;%20roll&#x21;.ogg</href>
  <propstat><prop><getcontentlength>8</getcontentlength></prop><status>HTTP/1.1 200 OK</status></propstat>
</response>
<response>
  <href><![CDATA[/remote.php/dav/A%20b/café.mp3]]></href>
  <propstat><prop><getcontentlength>9</getcontentlength></prop><status>HTTP/1.1 200 OK</status></propstat>
</response>
<response>
  <href>/remote.php/dav/A%20b/latin%FF.mp3</href>
  <propstat><prop><getcontentlength>10</getcontentlength></prop><status>HTTP/1.1 200 OK</status></propstat>
</response>
</multistatus>"#;
        assert_eq!(
            entries(xml).await.unwrap(),
            [
                folder(b"/"),
                folder(b"/A b"),
                file("/A b/片 100% # 1.flac".as_bytes(), 7),
                file(b"/A b/rock & roll!.ogg", 8),
                file("/A b/café.mp3".as_bytes(), 9),
                file(b"/A b/latin\xff.mp3", 10),
            ]
        );
        let entries = entries(xml).await.unwrap();
        assert_eq!(entries[2].name(), "片 100% # 1.flac".as_bytes());
        assert_eq!(entries[2].parent(), b"/A b");
        assert_eq!(entries[1].parent(), b"/");
    }

    #[tokio::test]
    async fn malformed_hrefs_and_failed_responses_are_errors() {
        for href in [
            "/elsewhere/a.mp3",
            "relative/a.mp3",
            "/remote.php/dav/a%2Fb.mp3",
            "/remote.php/dav/a//b.mp3",
            "/remote.php/dav/%2e%2e/a.mp3",
        ] {
            assert!(entries(&single(href)).await.is_err(), "{href} must fail");
        }
        assert!(entries(&single("/remote.php/dav/a.mp3")).await.is_ok());

        let failed = r#"<D:multistatus xmlns:D="DAV:"><D:response>
<D:href>/remote.php/dav/a.mp3</D:href><D:status>HTTP/1.1 403 Forbidden</D:status>
</D:response></D:multistatus>"#;
        assert!(entries(failed).await.unwrap_err().contains("403"));

        let bad_length = single("/remote.php/dav/a.mp3").replace(">1<", ">ten<");
        assert!(entries(&bad_length).await.is_err());
    }
    #[tokio::test]
    async fn bodies_that_end_inside_an_element_or_repeat_the_root_are_errors() {
        let complete = single("/remote.php/dav/a.mp3");
        let unclosed_root = complete.replace("</D:multistatus>", "");
        assert!(entries(&unclosed_root).await.is_err());
        assert!(entries("<D:multistatus xmlns:D=\"DAV:\">").await.is_err());
        let two_roots = format!("{complete}{complete}");
        assert!(entries(&two_roots).await.is_err());
    }

    #[tokio::test]
    async fn oversized_tokens_and_property_values_are_errors() {
        let junk = "a".repeat(4 * TOKEN_LIMIT);
        let ignored = format!(
            "<D:multistatus xmlns:D=\"DAV:\"><x:junk xmlns:x=\"urn:x\">{junk}</x:junk></D:multistatus>"
        );
        assert!(entries(&ignored).await.unwrap_err().contains("exceeds"));

        let href = format!("/remote.php/dav/{}", "&amp;".repeat(TOKEN_LIMIT));
        assert!(
            entries(&single(&href))
                .await
                .unwrap_err()
                .contains("exceeds")
        );

        let at_limit = format!("/remote.php/dav/{}", "b".repeat(255));
        assert!(entries(&single(&at_limit)).await.is_ok());
    }
    #[tokio::test]
    async fn nesting_deeper_than_the_limit_is_an_error() {
        let nested = |depth: usize| {
            format!(
                "<D:multistatus xmlns:D=\"DAV:\">{}{}</D:multistatus>",
                "<x:n xmlns:x=\"urn:x\">".repeat(depth),
                "</x:n>".repeat(depth)
            )
        };
        assert!(entries(&nested(DEPTH_LIMIT - 1)).await.is_ok());
        assert!(
            entries(&nested(DEPTH_LIMIT))
                .await
                .unwrap_err()
                .contains("deeper")
        );
    }
}
