use std::thread;

use clap::Parser;
use tracing::{debug, error, info, Instrument, trace};
use rust_vnc::{Error, protocol};
use rust_vnc::protocol::{C2S, ClientInit, Encoding, Message};
use my_vnc::dxgl::DisplayDuplWrapper;
use my_vnc::network_stream::{stream_factory_loop, TryClone, VncStream};
use my_vnc::server_connection::ServerConnection;
use my_vnc::server_events::input;
use my_vnc::server_state::ServerState;
use my_vnc::settings;
use my_vnc::settings::PIXEL_FORMAT;

#[derive(Parser, Debug)]
#[command(version, about, long_about = "A VNC server written in Rust")]
struct Args {
    #[arg(long, default_value = "localhost")]
    host: String,
    #[arg(short, long, default_value_t = 5900, env = "PORT")]
    port: u16,
    #[arg(short, long, default_value_t = 0, env = "DISPLAY")]
    display: u16,
    #[arg(short, long, default_value_t = false, env = "USE_TUNNELLING")]
    use_tunnelling: bool,
}

#[tokio::main(flavor = "multi_thread")]
#[tracing::instrument(level = "info")]
async fn main() {
    let args = Args::parse();
    println!("init logger for server Cargo version: {}", env!("CARGO_PKG_VERSION"));
    settings::init_logger();
    info!("args: {:?}", args);

    let bind = format!("{}:{}", args.host, args.port);
    let mut connection_id = 0;
    let result = stream_factory_loop(bind.as_str(), args.use_tunnelling, |stream| {
        let span = tracing::span!(tracing::Level::INFO, "connection", %connection_id);
        connection_id += 1;
        tokio::spawn(async move {
            info!("Connection established! {}", connection_id);
            match handle_client(stream, args.display) {
                Ok(_) => {
                    info!("Connection {} closed", connection_id);
                }
                Err(e) => {
                    info!("Connection {} closed with error: {:?}", connection_id, e);
                }
            }
        }.instrument(span));
    }).await;
    if let Err(e) = result {
        error!("Failed to start server: {:?}", e);
    } else {
        info!("Server terminated");
    }
}

#[tracing::instrument(level = "info", skip_all)]
fn handle_client(mut vnc_stream: Box<dyn VncStream>, display: u16) -> anyhow::Result<()> {
    let version = protocol::Version::Rfb38;
    info!("server version: {:?}", version);
    version.write_to(&mut vnc_stream)?;
    let client_version = protocol::Version::read_from(&mut vnc_stream)?;
    if client_version != version {
        anyhow::bail!("client version: {:?}", client_version);
    }
    info!("client version: {:?}", client_version);
    protocol::SecurityTypes(vec![protocol::SecurityType::None]).write_to(&mut vnc_stream)?;

    let client_security_type = protocol::SecurityType::read_from(&mut vnc_stream)?;
    if client_security_type != protocol::SecurityType::None {
        error!("client security type: {:?}", client_security_type);
        anyhow::bail!("client security type: {:?}", client_security_type);
    }
    info!("client security type: {:?}", client_security_type);

    protocol::SecurityResult::Succeeded.write_to(&mut vnc_stream)?;

    let client_init: ClientInit = protocol::ClientInit::read_from(&mut vnc_stream)?;
    info!("client init: {:?}", client_init);
    let mut display_duplicator = DisplayDuplWrapper::new(display)?;
    let (framebuffer_width, framebuffer_height) = display_duplicator.get_dimensions()?;

    let server_init = protocol::ServerInit {
        framebuffer_width,
        framebuffer_height,
        pixel_format: PIXEL_FORMAT,
        name: "rust-vnc".to_string(),
    };
    server_init.write_to(&mut vnc_stream)?;
    let tcp_stream_copy = vnc_stream.try_clone()?;
    let server_state = ServerState::new();
    thread::scope(|s| -> anyhow::Result<()>
    {
        let mut server_connection =
            ServerConnection::new(tcp_stream_copy, &server_state, &mut display_duplicator);
        let span = tracing::span!(tracing::Level::INFO, "server_loop");
        
        s.spawn(move || {
            span.in_scope(|| {
                let result = server_connection.update_frame_loop();
                if let Err(e) = result {
                    if let Some(Error::Disconnected) = e.downcast_ref() {
                        info!("client disconnected");
                    } else {
                        error!("Failed to update frame: {:?}", e);
                    }
                }
            });
        });
        let loop_result = server_loop(vnc_stream, &server_state);
        if let Err(e) = loop_result {
            error!("Failed to handle message: {:?}", e);
        }
        server_state.set_terminating();
        Ok(())
    })?;
    Ok(())
}

#[tracing::instrument(level = "info", skip_all)]
fn server_loop(mut tcp_stream: Box<dyn VncStream>, server_state: &ServerState) -> anyhow::Result<()> {
    loop {
        let message_result: rust_vnc::Result<C2S> = C2S::read_from(&mut tcp_stream);
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
                if encs.contains(&Encoding::Known(protocol::KnownEncoding::Zlib)) {
                    server_state.set_frame_encoding(Encoding::Known(protocol::KnownEncoding::Zlib));
                    info!("set frame encoding: {:?}", server_state.get_frame_encoding());
                }
            }
            C2S::FramebufferUpdateRequest { incremental, x_position, y_position, width, height } => {
                debug!("framebuffer update request: incremental: {}, x_position: {}, y_position: {}, width: {}, height: {}, frame: {:?}
                    ", incremental, x_position, y_position, width, height, server_state.get_frame());
                server_state.set_ready();
            }
            C2S::KeyEvent { down, key } => {
                info!("key event: down: {}, key: {}", down, key);
                let c2s = input::handle_key_event(down, key,
                                                  |key| { server_state.get_last_key_input(key) });
                if let C2S::KeyEvent { down, key } = c2s {
                    server_state.set_last_key_input(key, down);
                }
            }
            C2S::PointerEvent { x_position, y_position, button_mask } => {
                trace!("pointer event: x_position: {}, y_position: {}, button_mask: {:?}", x_position, y_position, button_mask);
                input::handle_pointer_event(server_state, message);
            }
            C2S::CutText(text) => {
                info!("cut text: {:?}", text);
                input::handle_clipboard_paste(text)
                    .unwrap_or_else(|e| error!("Failed to paste clipboard: {:?}", e));
            }
        }
    };
}
