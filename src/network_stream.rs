use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::Sender;
use log::{info, warn};
use tungstenite::ClientRequestBuilder;
use tungstenite::http::Uri;
use tungstenite::stream::MaybeTlsStream;

type WebSocket = tungstenite::WebSocket<MaybeTlsStream<TcpStream>>;

struct TunneledTcpStream {
    ws_stream: WebSocket,
}

impl TunneledTcpStream {
    fn new(tunnel_host: &str) -> anyhow::Result<TunneledTcpStream> {
        let req = ClientRequestBuilder::new(Uri::from_str(tunnel_host)?);
        let ws_stream: WebSocket = tungstenite::client::connect(req)?.0;
        Ok(TunneledTcpStream { ws_stream })
    }
}

impl Read for TunneledTcpStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let msg = self.ws_stream.read().map_err(|e| {
            warn!("TunneledTcpStream: {:?}", e);
            std::io::Error::new(std::io::ErrorKind::Other, e)
        })?;
        match msg {
            tungstenite::Message::Binary(data) => {
                let len = data.len();
                buf[..len].copy_from_slice(&data);
                Ok(len)
            }
            _ => {
                warn!("TunneledTcpStream unexpected message: {:?}", msg);
                Err(std::io::Error::new(std::io::ErrorKind::Other, "unexpected message"))
            }
        }
    }
}

impl Write for TunneledTcpStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.ws_stream.write(tungstenite::Message::Binary(Vec::from(buf))).map_err(|e| {
            warn!("TunneledTcpStream: {:?}", e);
            std::io::Error::new(std::io::ErrorKind::Other, e)
        })?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.ws_stream.flush().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }
}

pub struct CloneableTunneledTcpStream {
    tunneled_tcp_stream: Arc<Mutex<TunneledTcpStream>>,
    traffic_sender: Sender<()>,
    notified: bool,
}

impl CloneableTunneledTcpStream {
    fn new(tunnel_host: &str, sender: Sender<()>) -> anyhow::Result<CloneableTunneledTcpStream> {
        let tunneled_tcp_stream = Arc::new(Mutex::new(TunneledTcpStream::new(tunnel_host)?));
        Ok(CloneableTunneledTcpStream { tunneled_tcp_stream, traffic_sender: sender, notified: false })
    }
}

impl Read for CloneableTunneledTcpStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let result = self.tunneled_tcp_stream.lock().unwrap().read(buf);
        if !self.notified {
            self.traffic_sender.send(()).unwrap();
            self.notified = true;
            info!("notified");
        }
        result
    }
}

impl Write for CloneableTunneledTcpStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.tunneled_tcp_stream.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.tunneled_tcp_stream.lock().unwrap().flush()
    }
}

impl CloneableStream for CloneableTunneledTcpStream {}

impl TryClone for CloneableTunneledTcpStream {
    fn try_clone(&self) -> anyhow::Result<Self> {
        Ok(CloneableTunneledTcpStream {
            tunneled_tcp_stream: self.tunneled_tcp_stream.clone(),
            traffic_sender: self.traffic_sender.clone(),
            notified: true,
        })
    }
}
pub trait BoxTryClone {
    fn box_try_clone(&self) -> anyhow::Result<Box<dyn CloneableStream>>;
}
impl<T> BoxTryClone for T
where
    T: 'static + CloneableStream,
{
    fn box_try_clone(&self) -> anyhow::Result<Box<dyn CloneableStream>> {
        Ok(Box::new(self.try_clone()?))
    }
}

pub trait CloneableStream: TryClone + Read + Write + Send + BoxTryClone {}

pub trait TryClone {
    fn try_clone(&self) -> anyhow::Result<Self>
    where
        Self: Sized;
}
impl CloneableStream for TcpStream {}

impl TryClone for TcpStream {
    fn try_clone(&self) -> anyhow::Result<Self> {
        Ok(self.try_clone()?)
    }
}

impl TryClone for Box<dyn CloneableStream> {
    fn try_clone(&self) -> Result<Box<dyn CloneableStream>, anyhow::Error>
    {
        Ok(self.box_try_clone()? as Box<dyn CloneableStream>)
    }
}

pub fn stream_factory_loop(bind: &str, use_tunnelling: bool, mut on_stream: impl FnMut(Box<dyn CloneableStream>)) -> anyhow::Result<Sender<()>> {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    if use_tunnelling {
        let tunnel_host = format!("ws://{}", bind);
        loop {
            let tunneled_tcp_stream = CloneableTunneledTcpStream::new(&tunnel_host, tx.clone())?;
            on_stream(Box::new(tunneled_tcp_stream));
            rx.recv()?;
        }
    } else {
        let tcp_listener = TcpListener::bind(bind)?;
        for stream in tcp_listener.incoming() {
            let tcp_stream = stream?;
            on_stream(Box::new(tcp_stream));
        }
    }
    Ok(tx.clone())
}