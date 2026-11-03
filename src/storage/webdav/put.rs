use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use reqwest::header::CONTENT_LENGTH;
use reqwest::{Body, Method};
use tokio::io::{AsyncWrite, DuplexStream};
use tokio::task::JoinHandle;
use tokio_util::io::ReaderStream;

use super::client::WebDav;

const PIPE_BYTES: usize = 64 * 1024;

pub(super) struct PutWriter {
    pipe: DuplexStream,
    put: JoinHandle<Result<(), String>>,
    outcome: Option<Result<(), String>>,
    path: String,
    size: u64,
    written: u64,
}

impl PutWriter {
    pub(super) fn start(client: &WebDav, path: String, size: u64) -> PutWriter {
        let (pipe, body) = tokio::io::duplex(PIPE_BYTES);
        let client = client.clone();
        let target = path.clone();
        let put = tokio::spawn(async move {
            let body = Body::wrap_stream(ReaderStream::with_capacity(body, PIPE_BYTES));
            let answer = client
                .stream(Method::PUT, target.as_bytes(), |request| {
                    request.header(CONTENT_LENGTH, size).body(body)
                })
                .await
                .map_err(|error| error.to_string())?;
            if answer.status().is_success() {
                return Ok(());
            }
            Err(answer.refused().await.to_string())
        });
        PutWriter {
            pipe,
            put,
            outcome: None,
            path,
            size,
            written: 0,
        }
    }

    fn poll_outcome(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), String>> {
        if let Some(outcome) = &self.outcome {
            return Poll::Ready(outcome.clone());
        }
        let outcome = match ready!(Pin::new(&mut self.put).poll(cx)) {
            Ok(outcome) => outcome,
            Err(error) => Err(format!("PUT {} stopped: {error}", self.path)),
        };
        self.outcome = Some(outcome.clone());
        Poll::Ready(outcome)
    }
}

impl AsyncWrite for PutWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let writer = self.get_mut();
        if let Poll::Ready(outcome) = writer.poll_outcome(cx) {
            let error = match outcome {
                Ok(()) => format!("PUT {} answered before its body was sent", writer.path),
                Err(error) => error,
            };
            return Poll::Ready(Err(io::Error::other(error)));
        }
        match ready!(Pin::new(&mut writer.pipe).poll_write(cx, buf)) {
            Ok(count) => {
                writer.written += count as u64;
                Poll::Ready(Ok(count))
            }
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Poll::Pending,
            Err(error) => Poll::Ready(Err(error)),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let writer = self.get_mut();
        if writer.written != writer.size {
            writer.put.abort();
            return Poll::Ready(Err(io::Error::other(format!(
                "PUT {} aborted after {} of {} bytes",
                writer.path, writer.written, writer.size
            ))));
        }
        ready!(Pin::new(&mut writer.pipe).poll_shutdown(cx))?;
        writer.poll_outcome(cx).map_err(io::Error::other)
    }
}

impl Drop for PutWriter {
    fn drop(&mut self) {
        self.put.abort();
    }
}
