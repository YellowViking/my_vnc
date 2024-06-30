use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::spawn;
use log::{info, trace, warn};
use my_vnc::settings::init_logger;
use tungstenite;

fn main() {
    println!("init logger for tunnel");
    init_logger();
    let server = TcpListener::bind("0.0.0.0:80").unwrap();
    for stream in server.incoming() {
        let result = handle_tunnel_connect(stream.unwrap());
        if let Err(e) = result {
            warn!("error: {:?}... terminating", e);
        }
    }
}

fn handle_tunnel_connect(stream: TcpStream) -> anyhow::Result<()> {
    let ws_stream = Arc::new(Mutex::new(tungstenite::accept(stream)?));
    let proxy_server = TcpListener::bind("localhost:5900")?;
    info!("proxy server listening on port 5900");
    let mut proxy_stream = proxy_server.incoming().next().ok_or(anyhow::anyhow!("no proxy connection"))??;
    let ws_stream_for_thread = ws_stream.clone();
    let mut proxy_stream_for_thread = proxy_stream.try_clone()?;
    spawn(move || -> anyhow::Result<()> {
        loop {
            let mut buf = vec![0u8; 1024];
            proxy_stream_for_thread.read(&mut buf)?;
            let len = buf.len();
            ws_stream_for_thread.lock().unwrap().write(tungstenite::Message::Binary(buf))?;
            trace!("proxy -> ws: {}", len);
        }
    });

    loop {
        let msg = ws_stream.lock().unwrap().read()?;
        match msg {
            tungstenite::Message::Binary(data) => {
                proxy_stream.write(&data)?;
                trace!("ws -> proxy: {}", data.len());
            }
            _ => {
                warn!("unexpected message: {:?}", msg);
            }
        }
    }
}