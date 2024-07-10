use std::net::SocketAddr;
use std::ops::Not;

use anyhow::anyhow;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use log::error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::select;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::instrument;
use tracing::{event, info, info_span, trace, warn, Instrument};

use my_vnc::network_stream::TUNNEL_CONNECT;
use my_vnc::settings::init_logger;

const LISTEN_BIND: &'static str = "0.0.0.0:80";
type WSStream = WebSocketStream<TcpStream>;
#[tokio::main]
#[instrument(level = "info")]
async fn main() {
    println!("init logger for tunnel");
    init_logger();
    info!("start tunnel listener {}", LISTEN_BIND);
    let server = TcpListener::bind(LISTEN_BIND).await.unwrap();
    let mut proxy_server = TcpListener::bind("localhost:5900").await.unwrap();
    let (proxy_client_tx, mut proxy_client_rx) =
        tokio::sync::mpsc::channel::<(WSStream, tracing::Span)>(1);
    let mut tunnel_id = 0;

    tokio::spawn(async move {
        loop {
            info!("waiting for tunnel connection channel message");
            let (ws_stream, span) = proxy_client_rx.recv().await.unwrap();
            let result = handle_client_connect(&mut proxy_server, ws_stream)
                .instrument(span)
                .await;
            if result.is_err() {
                error!("handle_client_connect error: {:?}", result.err());
            }
        }
    });

    loop {
        let span = info_span!("main_loop", tunnel_id);
        let _enter = span.enter();
        {
            tunnel_id += 1;
            let proxy_client_tx = proxy_client_tx.clone();
            info!("waiting for tunnel connection");
            let (stream, tunnel_socket_addr) = server.accept().await.unwrap();
            let ws_stream = tokio_tungstenite::accept_async(stream).await;
            if ws_stream.is_err() {
                error!("error: {:?}", ws_stream.err());
                continue;
            }
            info!("new tunnel connection: {:?}", tunnel_socket_addr);
            let ws_stream = ws_stream.unwrap();
            proxy_client_tx
                .send((ws_stream, span.clone()))
                .await
                .unwrap();
        }
    }
}

#[instrument(level = "info", fields(tunnel_id), skip_all)]
async fn handle_client_connect(
    proxy_server: &mut TcpListener,
    ws_stream: WSStream,
) -> anyhow::Result<()> {
    info!("waiting for proxy connection");
    let ct = tokio_util::sync::CancellationToken::new();
    let task_ping_pong = ws_ping_pong_loop(ws_stream, ct.clone());
    tokio::pin!(task_ping_pong);
    let proxy_client = select! {
        _ = &mut task_ping_pong => {
            warn!("ws_ping_pong_loop terminated");
            return Err(anyhow!("ws_ping_pong_loop terminated"));
        }
        client = proxy_server.accept() => {
            ct.cancel();
            info!("proxy_server.accept() terminated");
            client
        }
    }?;
    let result = task_ping_pong.await?;
    let ws_stream = result?;
    let (proxy_stream, proxy_addr) = proxy_client;
    info!("new proxy connection: {:?}", proxy_addr);
    tokio::spawn(async move {
        let result = handle_tunnel_connect(ws_stream, proxy_stream, proxy_addr).await;
        if result.is_err() {
            error!("handle_tunnel_connect error: {:?}", result.err());
        }
    });
    Ok(())
}

#[instrument(level = "info", skip_all)]
fn ws_ping_pong_loop(
    mut ws_stream: WSStream,
    cancel: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<anyhow::Result<WSStream>> {
    tokio::spawn(async move {
        loop {
            info_span!("ws_ping_pong_loop");
            if cancel.is_cancelled() {
                info!("ws_ping_pong_loop: cancelled");
                return Ok(ws_stream);
            }
            ws_ping_pong(&mut ws_stream).await?;
            select! {
                 _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {}
                 _ = cancel.cancelled() => {
                     return Ok(ws_stream);
                 }
            }
        }
    })
}

#[instrument(level = "info", skip_all)]
async fn ws_ping_pong(ws_stream: &mut WSStream) -> anyhow::Result<()> {
    let block = async {
        ws_stream.send(Message::Ping(vec![])).await?;
        let msg = ws_stream.next().await.ok_or(anyhow!("no message"))??;
        if msg.is_pong().not() {
            return Err(anyhow!("ws_ping_pong: unexpected message: {:?}", msg));
        }
        Ok(())
    };
    select! {
        ret = block => {ret}
        _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
            Err(anyhow!("ws_ping_pong: timeout"))
        }
    }
}

#[instrument(level = "info", fields(socket_addr, tunnel_id), skip_all)]
async fn handle_tunnel_connect(
    mut ws_stream: WebSocketStream<TcpStream>,
    proxy_stream: TcpStream,
    socket_addr: SocketAddr,
) -> anyhow::Result<()> {
    let span = tracing::span!(tracing::Level::INFO, "handle_tunnel_connect", %socket_addr);
    let span_for_task = span.clone();

    info!("proxy connected: {:?}", socket_addr);
    ws_stream
        .send(Message::Text(TUNNEL_CONNECT.to_string()))
        .await?;

    let (mut ws_writer, mut ws_reader): (
        SplitSink<WebSocketStream<TcpStream>, Message>,
        SplitStream<WebSocketStream<TcpStream>>,
    ) = ws_stream.split();
    let (mut proxy_reader, mut proxy_writer): (OwnedReadHalf, OwnedWriteHalf) =
        proxy_stream.into_split();

    let task_out: tokio::task::JoinHandle<Result<(), anyhow::Error>> = tokio::spawn(
        async move {
            let mut buf = vec![0u8; 4096];
            loop {
                proxy_out(&mut ws_writer, &mut proxy_reader, &mut buf).await?
            }
        }
        .instrument(span),
    );

    let task_in: tokio::task::JoinHandle<Result<(), anyhow::Error>> = tokio::spawn(
        async move {
            let mut seq = 0;
            loop {
                proxy_in(&mut ws_reader, &mut proxy_writer, seq).await?;
                seq += 1;
            }
        }
        .instrument(span_for_task),
    );

    select! {
        e = task_out => {
            warn!("task_out terminated {:?}", e);
        }
        e = task_in => {
            warn!("task_in terminated {:?}", e);
        }
    }
    Ok(())
}

#[instrument(level = "trace", skip_all)]
async fn proxy_out(
    ws_writer: &mut SplitSink<WebSocketStream<TcpStream>, Message>,
    proxy_reader: &mut OwnedReadHalf,
    mut buf: &mut Vec<u8>,
) -> anyhow::Result<()> {
    let byte = proxy_reader.read(&mut buf).await?;
    if byte == 0 {
        warn!("proxy_reader.read() == 0");
        return Err(anyhow!("proxy_reader.read() == 0"));
    }
    event!(tracing::Level::TRACE, "proxy_out: read {} bytes", byte);
    trace!("proxy_out: read {} bytes", byte);
    ws_writer
        .send(Message::Binary(buf[..byte].to_vec()))
        .await?;
    Ok(())
}

#[instrument(level = "trace", skip_all)]
async fn proxy_in(
    ws_reader: &mut SplitStream<WebSocketStream<TcpStream>>,
    proxy_writer: &mut OwnedWriteHalf,
    seq: i32,
) -> anyhow::Result<()> {
    let byte = ws_reader.next().await.ok_or(anyhow!("no message"))??;
    match byte {
        Message::Binary(data) => {
            event!(tracing::Level::TRACE, proxy_in = data.len(), checksum = ?crc32fast::hash(&data), %seq);
            proxy_writer.write_all(&data).await?;
            proxy_writer.flush().await?;
            trace!("proxy_in: {} bytes written", data.len());
        }
        Message::Close(frame) => {
            warn!("proxy_in: closing {:?}", frame);
            return Err(anyhow!("proxy_in: closing"));
        }
        _ => {
            warn!("proxy_in: unexpected message: {:?}", byte);
            return Err(anyhow!("proxy_in: unexpected message"));
        }
    }
    Ok(())
}
