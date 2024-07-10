use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::io::{Error, Read, Write};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, Stream, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::{ClientRequestBuilder, Message};
use tokio_tungstenite::{tungstenite, MaybeTlsStream};
use tracing::instrument;
use tracing::{info, trace, warn};

type WebSocket = tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug)]
struct TunneledTcpStream {
    ws_reader: TunneledTcpStreamAsyncRead,
    ws_writer: TunneledTcpStreamAsyncWrite,
}

impl TunneledTcpStream {
    async fn new(tunnel_host: &str) -> anyhow::Result<TunneledTcpStream> {
        let req = ClientRequestBuilder::new(Uri::from_str(tunnel_host)?);
        info!("TunneledTcpStream: connecting to {}", tunnel_host);
        let (ws_stream, _) = tokio_tungstenite::connect_async(req).await?;
        info!("TunneledTcpStream: connected");
        let (mut sender, mut receiver) = ws_stream.split();
        info!("TunneledTcpStream: waiting for connect message");
        loop {
            let message = receiver
                .next()
                .await
                .ok_or(anyhow::anyhow!("no message"))??;
            match message {
                Message::Text(text) => {
                    if text == TUNNEL_CONNECT {
                        break;
                    } else {
                        warn!("TunneledTcpStream: unexpected message: {:?}", text);
                        return Err(anyhow::anyhow!("unexpected message"));
                    }
                }
                Message::Ping(_) => {
                    trace!("TunneledTcpStream: ping");
                    sender.send(Message::Pong(Vec::new())).await?;
                    continue;
                }
                _ => {
                    warn!("TunneledTcpStream: unexpected message: {:?}", message);
                    return Err(anyhow::anyhow!("unexpected message"));
                }
            }
        }
        info!("TunneledTcpStream: connected");
        let ws_stream = TunneledTcpStreamAsyncRead::new(receiver);
        Ok(TunneledTcpStream {
            ws_reader: ws_stream,
            ws_writer: TunneledTcpStreamAsyncWrite::new(sender),
        })
    }
}

#[derive(Debug)]
struct TunneledTcpStreamAsyncRead {
    buf: VecDeque<u8>,
    ws_stream: SplitStream<WebSocket>,
}
impl TunneledTcpStreamAsyncRead {
    pub fn new(ws_stream: SplitStream<WebSocket>) -> TunneledTcpStreamAsyncRead {
        TunneledTcpStreamAsyncRead {
            buf: VecDeque::new(),
            ws_stream,
        }
    }
}
impl AsyncRead for TunneledTcpStreamAsyncRead {
    #[instrument(level = "trace", skip_all, ret)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut ret = Ok(());
        // Check if internal buffer has enough data
        while self.buf.len() < buf.remaining() {
            let ws_stream = Pin::new(&mut self.ws_stream);
            let message = ws_stream.poll_next(cx).map_err(map_to_io_error)?;
            let message = ready!(message);
            let message = message
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no message"))?;
            tracing::event!(tracing::Level::TRACE, message_len = message.len(), message_type = ?message);
            match message {
                Message::Binary(data) => {
                    tracing::event!(tracing::Level::TRACE, data_len = data.len());
                    self.buf.extend(data);
                    ret = Ok(());
                }
                Message::Close(_) => {
                    warn!("TunneledTcpStream: closing");
                    ret = Err(io::Error::new(io::ErrorKind::ConnectionAborted, "close"));
                    break;
                }
                Message::Ping(_) => {
                    info!("TunneledTcpStream: ping");
                    break;
                }
                _ => {
                    warn!("TunneledTcpStream: unexpected message: {:?}", message);
                    ret = Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unexpected message",
                    ));
                    break;
                }
            }
        }
        let len = std::cmp::min(self.buf.len(), buf.remaining());
        tracing::event!(tracing::Level::TRACE, buf_len = len);
        buf.put_slice(&self.buf.drain(..len).collect::<Vec<u8>>().as_slice());
        Poll::Ready(ret)
    }
}

#[derive(Debug)]
struct TunneledTcpStreamAsyncWrite {
    sink: SplitSink<WebSocket, tungstenite::Message>,
    seq: u32,
}

impl TunneledTcpStreamAsyncWrite {
    pub fn new(sink: SplitSink<WebSocket, tungstenite::Message>) -> TunneledTcpStreamAsyncWrite {
        TunneledTcpStreamAsyncWrite { sink, seq: 0 }
    }
}

impl AsyncWrite for TunneledTcpStreamAsyncWrite {
    #[instrument(level = "trace", skip_all, ret, fields(buf_len = buf.len()))]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        ready!(self.sink.poll_ready_unpin(cx)).map_err(map_to_io_error)?;
        let message = tungstenite::Message::Binary(Vec::from(buf));
        self.sink
            .start_send_unpin(message)
            .map_err(map_to_io_error)?;
        let poll = self.sink.poll_flush_unpin(cx).map_err(map_to_io_error)?;
        if poll.is_pending() {
            cx.waker().wake_by_ref();
        }
        Poll::Ready(Ok(buf.len()))
    }
    #[instrument(level = "trace", skip_all, ret)]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        trace!("TunneledTcpStream: flushing");
        let block = async {
            self.sink.flush().await.map_err(|e| {
                warn!("TunneledTcpStream: {:?}", e);
                Error::new(std::io::ErrorKind::Other, e)
            })?;
            Ok(())
        };
        tokio::pin!(block);
        block.poll(cx)
    }

    #[instrument(level = "trace", skip_all, ret)]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        trace!("TunneledTcpStream: shutting down");
        let block = async {
            self.sink.close().await.map_err(|e| {
                warn!("TunneledTcpStream: {:?}", e);
                Error::new(std::io::ErrorKind::Other, e)
            })?;
            Ok(())
        };
        tokio::pin!(block);
        block.poll(cx)
    }
}

pub trait BoxTryClone {
    fn box_try_clone(&self) -> anyhow::Result<Box<dyn VncStream + 'static>>;
}
impl<T> BoxTryClone for T
where
    T: 'static + VncStream,
{
    fn box_try_clone(&self) -> anyhow::Result<Box<dyn VncStream + 'static>> {
        Ok(Box::new(self.try_clone()?))
    }
}

pub trait VncStream: TryClone + Read + Write + Send + BoxTryClone {}

pub trait TryClone {
    fn try_clone(&self) -> anyhow::Result<Self>
    where
        Self: Sized;
}

impl TryClone for Box<dyn VncStream + 'static> {
    fn try_clone(&self) -> Result<Box<dyn VncStream>, anyhow::Error> {
        self.box_try_clone()
    }
}

enum WriterMessage {
    Write(Vec<u8>),
    Flush,
}
enum WriterReply {
    Write(io::Result<usize>),
    Flush(io::Result<()>),
}
#[derive(Debug)]
pub struct CloneableStream {
    reader: Arc<Mutex<(SyncSender<usize>, Receiver<io::Result<Vec<u8>>>)>>,
    writer: Arc<Mutex<(SyncSender<WriterMessage>, Receiver<WriterReply>)>>,
}

impl TryClone for CloneableStream {
    fn try_clone(&self) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        Ok(CloneableStream {
            reader: self.reader.clone(),
            writer: self.writer.clone(),
        })
    }
}

impl VncStream for CloneableStream {}
impl CloneableStream {
    fn new<T, R>(mut reader: T, mut writer: R) -> CloneableStream
    where
        T: AsyncRead + Unpin + Send + 'static,
        R: AsyncWrite + Unpin + Send + 'static,
    {
        let (reader_tx, reader_rx) = std::sync::mpsc::sync_channel::<usize>(1);
        let (reader_reply_tx, reader_reply_rx) =
            std::sync::mpsc::sync_channel::<io::Result<Vec<u8>>>(1);
        let (writer_tx, writer_rx) = std::sync::mpsc::sync_channel::<WriterMessage>(1);
        let (writer_reply_tx, writer_reply_rx) = std::sync::mpsc::sync_channel::<WriterReply>(1);
        tokio::spawn(async move {
            loop {
                let size = reader_rx.recv().unwrap();
                let mut buf = vec![0u8; size];
                let result = reader.read(&mut buf).await;
                reader_reply_tx
                    .send(result.map(|bytesize| buf[..bytesize].to_vec()))
                    .unwrap();
            }
        });
        tokio::spawn(async move {
            loop {
                let message = writer_rx.recv().unwrap();
                match message {
                    WriterMessage::Write(buf) => {
                        let result = writer.write(&buf).await;
                        writer_reply_tx.send(WriterReply::Write(result)).unwrap();
                    }
                    WriterMessage::Flush => {
                        let result = writer.flush().await;
                        writer_reply_tx.send(WriterReply::Flush(result)).unwrap();
                    }
                }
            }
        });
        CloneableStream {
            reader: Arc::new(Mutex::new((reader_tx, reader_reply_rx))),
            writer: Arc::new(Mutex::new((writer_tx, writer_reply_rx))),
        }
    }
}

fn map_to_io_error<T>(e: T) -> io::Error
where
    T: std::error::Error + 'static + Send + Sync,
{
    io::Error::new(io::ErrorKind::Other, e)
}
impl Read for CloneableStream {
    #[instrument(level = "trace", skip(self, buf), fields(buf_len = buf.len()))]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        {
            self.reader
                .lock()
                .unwrap()
                .0
                .send(buf.len())
                .map_err(map_to_io_error)?;
        }
        let ret = self
            .reader
            .lock()
            .unwrap()
            .1
            .recv()
            .map_err(map_to_io_error)??;
        let len = ret.len();
        buf[..len].copy_from_slice(&ret);
        Ok(len)
    }
}

impl Write for CloneableStream {
    #[instrument(level = "trace", skip(self, buf), fields(buf_len = buf.len()))]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        {
            self.writer
                .lock()
                .unwrap()
                .0
                .send(WriterMessage::Write(Vec::from(buf)))
                .map_err(map_to_io_error)?;
        }
        match self
            .writer
            .lock()
            .unwrap()
            .1
            .recv()
            .map_err(map_to_io_error)?
        {
            WriterReply::Write(result) => result,
            _ => panic!("unexpected reply"),
        }
    }

    #[instrument(level = "trace", skip(self))]
    fn flush(&mut self) -> io::Result<()> {
        {
            self.writer
                .lock()
                .unwrap()
                .0
                .send(WriterMessage::Flush)
                .map_err(map_to_io_error)?;
        }
        match self
            .writer
            .lock()
            .unwrap()
            .1
            .recv()
            .map_err(map_to_io_error)?
        {
            WriterReply::Flush(result) => result,
            _ => panic!("unexpected reply"),
        }
    }
}

#[instrument(level = "info", skip_all, fields(bind = bind, use_tunnelling = use_tunnelling))]
pub async fn stream_factory_loop(
    bind: &str,
    use_tunnelling: bool,
    mut on_stream: impl FnMut(CloneableStream),
) -> anyhow::Result<()> {
    if use_tunnelling {
        let tunnel_host = format!("ws://{}", bind);
        loop {
            let result = TunneledTcpStream::new(tunnel_host.as_str()).await;
            if let Err(e) = result {
                warn!("Failed to connect to tunnel: {:?}", e);
                continue;
            }
            let tunneled_tcp_stream = result.unwrap();
            let stream =
                CloneableStream::new(tunneled_tcp_stream.ws_reader, tunneled_tcp_stream.ws_writer);
            on_stream(stream);
        }
    } else {
        let tcp_listener = tokio::net::TcpListener::bind(bind).await?;
        loop {
            let (socket, addr) = tcp_listener.accept().await?;
            info!("Connection established! {:?}", addr);
            let (reader, writer) = socket.into_split();
            let stream = CloneableStream::new(reader, writer);
            on_stream(stream);
        }
    }
}

pub const TUNNEL_CONNECT: &str = "TUNNEL-CONNECT";
