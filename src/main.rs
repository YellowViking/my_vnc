use std::ffi::OsString;
use clap::Parser;
use log::{debug, error, info, trace};
use win_desktop_duplication::*;
use win_desktop_duplication::{devices::*, tex_reader::*};
use std::io::Write;
use std::net::TcpStream;
use std::sync::{Arc, LockResult, RwLock};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::mpsc::channel;
use std::thread;
use std::thread::{Scope, sleep, spawn};
use anyhow::{anyhow, bail};
use rust_vnc_lib::{Error, protocol, Rect};
use rust_vnc_lib::Error::Server;
use rust_vnc_lib::protocol::{C2S, ClientInit, Message, S2C};
use windows::core::Interface;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::Dxgi::IDXGISurface1;
use windows::Win32::Graphics::Gdi;
use server_connection::{ServerConnection, ServerState};
use settings::PIXEL_FORMAT;
use win_desktop_duplication::outputs::Display;
use crate::dxgl::{DisplayDuplWrapper};

mod server_connection;
mod dxgl;
mod settings;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = "localhost")]
    host: String,
    #[arg(short, long, default_value_t = 5900, env = "PORT")]
    port: u16,
    #[arg(short, long, default_value_t = 0, env = "DISPLAY")]
    display: u16,
}

fn main() {
    let args = Args::parse();
    println!("init logger");
    env_logger::Builder::from_default_env()
        .format(|buf, record| {
            writeln!(
                buf,
                "{}:{} {} [{}] - {}",
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
                record.level(),
                record.args()
            )
        })
        .init();
    info!("args: {:?}", args);

    let bind = format!("{}:{}", args.host, args.port);
    let listener
        = std::net::TcpListener::bind(bind).unwrap();
    info!("Listening on port {}", listener.local_addr().unwrap().port());
    let mut connection_id = 0;
    for stream in listener.incoming() {
        let stream = stream.unwrap();
        thread::spawn(move || {
            match handle_client(stream, args.display) {
                Ok(_) => {
                    println!("Connection {} closed", connection_id);
                }
                Err(e) => {
                    println!("Connection {} closed with error: {:?}", connection_id, e);
                }
            }
        });
        println!("Connection established! {}", connection_id);
        connection_id += 1;
    }
}

fn handle_client(mut tcp_stream: TcpStream, display: u16) -> anyhow::Result<()> {
    let version = protocol::Version::Rfb38;
    version.write_to(&mut tcp_stream)?;
    let client_version = protocol::Version::read_from(&mut tcp_stream)?;
    if client_version != version {
        anyhow::bail!("client version: {:?}", client_version);
    }
    info!("client version: {:?}", client_version);
    protocol::SecurityTypes(vec![protocol::SecurityType::None]).write_to(&mut tcp_stream)?;

    let client_security_type = protocol::SecurityType::read_from(&mut tcp_stream)?;
    if client_security_type != protocol::SecurityType::None {
        error!("client security type: {:?}", client_security_type);
        anyhow::bail!("client security type: {:?}", client_security_type);
    }
    info!("client security type: {:?}", client_security_type);

    protocol::SecurityResult::Succeeded.write_to(&mut tcp_stream)?;

    let client_init: ClientInit = protocol::ClientInit::read_from(&mut tcp_stream)?;
    info!("client init: {:?}", client_init);
    let mut display_duplicator = DisplayDuplWrapper::new(display)?;
    let (framebuffer_width, framebuffer_height) = display_duplicator.get_dimensions()?;

    let server_init = protocol::ServerInit {
        framebuffer_width,
        framebuffer_height,
        pixel_format: PIXEL_FORMAT,
        name: "rust-vnc".to_string(),
    };
    server_init.write_to(&mut tcp_stream)?;
    let server_state = ServerState::new();
    let mut server_connection =
        ServerConnection::new(tcp_stream.try_clone()?, &server_state, &mut display_duplicator);

    thread::scope(|s| -> anyhow::Result<()>
    {
        s.spawn(move || {
            let result = server_connection.update_frame_loop();
            if let Err(e) = result {
                if let Some(Error::Disconnected) = e.downcast_ref() {
                    info!("client disconnected");
                } else {
                    error!("Failed to update frame: {:?}", e);
                }
            }
        });
        let loop_result = server_loop(&mut tcp_stream, &server_state);
        if let Err(e) = loop_result {
            error!("Failed to handle message: {:?}", e);
        }
        server_state.set_terminating();
        Ok(())
    })?;
    Ok(())
}

fn server_loop(mut tcp_stream: &mut TcpStream, server_state: &ServerState) -> anyhow::Result<()> {
    loop {
        let message_result: rust_vnc_lib::Result<C2S> = C2S::read_from(&mut tcp_stream);
        if let Err(Error::Disconnected) = message_result {
            return Ok(());
        }
        let message = message_result?;
        match message {
            C2S::SetPixelFormat(format) => {
                info!("set pixel format: {:?}", format);
            }
            C2S::SetEncodings(encs) => {
                info!("set encodings: {:?}", encs);
            }
            C2S::FramebufferUpdateRequest { incremental, x_position, y_position, width, height } => {
                debug!("framebuffer update request: incremental: {}, x_position: {}, y_position: {}, width: {}, height: {}, frame: {:?}
                    ", incremental, x_position, y_position, width, height, server_state.get_frame());
                server_state.set_ready();
            }
            C2S::KeyEvent { down, key } => {
                info!("key event: down: {}, key: {}", down, key);
            }
            C2S::PointerEvent { x_position, y_position, button_mask } => {
                trace!("pointer event: x_position: {}, y_position: {}, button_mask: {}", x_position, y_position, button_mask);
            }
            C2S::CutText(text) => {
                info!("cut text: {:?}", text);
            }
        }
    };
}
