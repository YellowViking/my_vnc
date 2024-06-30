use std::io::{Error, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn};
use anyhow::__private::kind::TraitKind;
use anyhow::anyhow;
use log::{info, trace, warn};
use my_vnc::settings::init_logger;
use tungstenite;
use tungstenite::{Error, Message, WebSocket};
use my_vnc::network_stream;
use my_vnc::network_stream::TUNNEL_CONNECT;

fn main() {
    println!("init logger for tunnel");
    init_logger();
    let server = TcpListener::bind("0.0.0.0:80").unwrap();
    for stream in server.incoming() {
        info!("new connection");
        let result = handle_tunnel_connect(stream.unwrap());
        if let Err(e) = result {
            warn!("error: {:?}... terminating", e);
        }
    }
}

fn handle_tunnel_connect(ws_tcp_stream: TcpStream) -> anyhow::Result<()> {
    ws_tcp_stream.set_nonblocking(true)?;
    let ws_stream = Arc::new(Mutex::new(tungstenite::accept(ws_tcp_stream)?));
    let proxy_server = TcpListener::bind("localhost:5900")?;
    info!("proxy server listening on port 5900");
    let mut proxy_stream = proxy_server.incoming().next().ok_or(anyhow::anyhow!("no proxy connection"))??;
    info!("proxy connected");
    { ws_stream.lock().unwrap().send(tungstenite::Message::Text(TUNNEL_CONNECT.to_string()))?; }
    let ws_stream_for_thread = ws_stream.clone();
    let mut proxy_stream_for_thread = proxy_stream.try_clone()?;
    let t1 = spawn(move || -> anyhow::Result<()> {
        loop {
            let mut buf = vec![0u8; 1024];
            trace!("proxy -> ws: reading message");
            let byte = proxy_stream_for_thread.read(&mut buf)?;
            if byte == 0 {
                break;
            }
            trace!("proxy -> ws: {} received", byte);
            { ws_stream_for_thread.lock().unwrap().send(tungstenite::Message::Binary(buf[..byte].to_vec()))? };
            trace!("proxy -> ws: {} sending", byte)
        }
        ws_stream_for_thread.lock().unwrap().close(None)?;
        Err(anyhow::anyhow!("proxy -> ws: no more data"))
    });

    let t2 = spawn(move || -> anyhow::Result<()> {
        loop {
            trace!("ws -> proxy: waiting for message");
            let msg = network_stream::ws_stream_message_poll(|| ws_stream.lock().unwrap().read().map_err(map_would_block))?;
            let len = msg.len();
            trace!("ws -> proxy: {} sending", len);
            match msg {
                tungstenite::Message::Binary(data) => {
                    proxy_stream.write_all(&data)?;
                    proxy_stream.flush()?;
                    trace!("ws -> proxy: {}", data.len());
                }
                tungstenite::Message::Close(frame) => {
                    trace!("ws -> proxy: closing {:?}", frame);
                    break;
                }
                _ => {
                    warn!("unexpected message: {:?}", msg);
                }
            }
        }
        proxy_stream.shutdown(std::net::Shutdown::Both)?;
        Err(anyhow::anyhow!("ws -> proxy: no more data"))
    });
    //
    // t1.join().unwrap()?;
    // t2.join().unwrap()?;
    Ok(())
}

fn map_would_block(e: Error) -> Error {
    match e {
        tungstenite::Error::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::io::Error::new(std::io::ErrorKind::WouldBlock, e),
        _ => std::io::Error::new(std::io::ErrorKind::Other, e)
    }
}

