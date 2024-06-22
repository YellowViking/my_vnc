use std::ffi::c_void;
use std::net::TcpStream;
use log::{debug, error, info};
use std::{mem, thread};
use windows::Win32::Graphics::Dxgi::IDXGISurface1;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::Gdi;
use anyhow::bail;
use rust_vnc_lib::protocol::{Message, S2C};
use rust_vnc_lib::protocol;
use std::thread::sleep;
use win_desktop_duplication::DesktopDuplicationApi;
use win_desktop_duplication::outputs::Display;
use win_desktop_duplication::tex_reader::TextureReader;
use windows::core::Interface;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicPtr, AtomicUsize};
use windows::Win32::Graphics::Gdi::{BI_RGB, BitBlt, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, GetBitmapBits, GetDIBits, GetObjectA, GetObjectW, ROP_CODE, SelectObject};
use windows::Win32::UI::WindowsAndMessaging::{CURSORINFO, GetCursor, GetCursorInfo, GetIconInfo, ICONINFO};
use win_desktop_duplication::texture::Texture;
use crate::dxgl::DisplayDuplWrapper;

pub struct ServerConnection<'a> {
    tcp_stream: TcpStream,
    pic_data: Vec<u8>,
    server_state: &'a ServerState,
    display_dupl_wrapper: &'a mut DisplayDuplWrapper,
}

pub struct ServerState {
    frame: AtomicUsize,
    connection_state: AtomicI32,
    cursor_sent: AtomicIsize,
}

pub enum ConnectionState {
    Init = -1,
    Ready = 0,
    Terminating = 1,
}

impl<'a> ServerConnection<'a> {
    pub fn new(tcp_stream: TcpStream, server_state: &'a ServerState, display_dupl_wrapper: &'a mut DisplayDuplWrapper) -> Self {
        let pic_data: Vec<u8> = vec![0; 0];
        ServerConnection {
            tcp_stream,
            pic_data,
            server_state,
            display_dupl_wrapper,
        }
    }

    pub(crate) fn update_frame_loop(&mut self) -> anyhow::Result<()> {
        let duration = std::time::Duration::from_millis(1000 / 60);
        info!("update_frame loop started");
        loop {
            if self.server_state.get_terminating() {
                info!("terminating update_frame loop");
                return Ok(());
            }
            let start = std::time::Instant::now();
            if self.server_state.get_ready() {
                self.display_dupl_wrapper.copy_from_desktop()?;
                self.acquire_frame()?;
                self.send_frame()?;
            }
            let elapsed = start.elapsed();
            if duration > elapsed {
                sleep(duration - elapsed);
            }
            self.server_state.inc_frame();
        }
    }

    fn acquire_frame(&mut self) -> anyhow::Result<()> {
        // draw frame count on the hdc
        self.display_dupl_wrapper.draw_to_texture(|hdc| -> anyhow::Result<()> {
            let frame = self.server_state.frame.load(std::sync::atomic::Ordering::Relaxed);
            let text = format!("Frame: {}", frame);
            unsafe {
                Gdi::SetBkMode(hdc, Gdi::TRANSPARENT);
                match Gdi::TextOutA(hdc, 0, 0, text.as_str().as_ref()) {
                    BOOL(b) => {
                        if b == 0 {
                            bail!("Failed to draw text, error: {:?}", windows::Win32::Foundation::GetLastError())
                        }
                    }
                }
            };
            Ok(())
        })?;

        let result = self.display_dupl_wrapper.copy_to_vec();
        if let Err(e) = result {
            bail!("Failed to read texture data: {:?}", e);
        }
        self.pic_data = result?;
        Ok(())
    }

    fn send_frame(&mut self) -> anyhow::Result<()> {
        debug!("frame acquired: {} bytes", self.pic_data.len());
        let message = S2C::FramebufferUpdate {
            count: 1,
        };
        let (width, height) = self.display_dupl_wrapper.get_dimensions()?;
        let rect = protocol::Rectangle {
            x_position: 0,
            y_position: 0,
            width,
            height,
            encoding: protocol::Encoding::Raw,
        };
        message.write_to(&mut self.tcp_stream)?;
        rect.write_to(&mut self.tcp_stream)?;
        self.tcp_stream.flush()?;
        self.tcp_stream.write_all(&self.pic_data)?;
        self.tcp_stream.flush()?;
        Ok(())
    }

    fn send_cursor(&mut self) -> anyhow::Result<()> {
        unsafe {
            let mut cursor_info = CURSORINFO::default();
            cursor_info.cbSize = mem::size_of::<CURSORINFO>() as u32;
            GetCursorInfo(&mut cursor_info)?;
            let hcursor = cursor_info.hCursor;
            if self.server_state.get_cursor_sent() == hcursor.0 {
                return Ok(());
            }
            self.server_state.set_cursor_sent(hcursor.0);
            let mut icon_info = ICONINFO::default();
            GetIconInfo(hcursor, &mut icon_info)?;
            let hbmp = icon_info.hbmColor;
            let mut icon_bitmap = BITMAP::default();
            GetObjectW(hbmp, std::mem::size_of::<BITMAP>() as i32, Some(&icon_bitmap as *const _ as *mut c_void));
            let mut icon_bitmap_mask = BITMAP::default();
            GetObjectW(icon_info.hbmMask, std::mem::size_of::<BITMAP>() as i32, Some(&icon_bitmap_mask as *const _ as *mut c_void));

            info!("icon_info: {:?}, cursor_info: {:?}", icon_info, cursor_info);
            info!("icon_bitmap: {:?}", icon_bitmap);
            info!("icon_bitmap_mask: {:?}", icon_bitmap_mask);
            let mut cursor_pixels = vec![0; (icon_bitmap.bmWidth * icon_bitmap.bmHeight * 4) as usize];
            let bytes = GetBitmapBits(hbmp, cursor_pixels.len() as i32, cursor_pixels.as_mut_ptr() as *mut c_void);
            info!("icon_bitmap copied: {}", bytes);
            std::fs::File::create("icon.bin")?.write_all(&cursor_pixels)?;
            let mut mask_pixels = vec![0; (icon_bitmap_mask.bmWidth * icon_bitmap_mask.bmHeight / 8) as usize];
            let bytes = GetBitmapBits(icon_info.hbmMask, mask_pixels.len() as i32, mask_pixels.as_mut_ptr() as *mut c_void);
            // invert mask_pixels
            for i in 0..mask_pixels.len() {
                mask_pixels[i] = !mask_pixels[i];
            }
            info!("icon_bitmap_mask copied: {}", bytes);
            // dump bytes to a file
            let mut file = std::fs::File::create("mask.bin")?;
            file.write_all(&mask_pixels)?;

            let message = S2C::FramebufferUpdate {
                count: 1,
            };
            let rect = protocol::Rectangle {
                x_position: 0,
                y_position: 0,
                width: icon_bitmap.bmWidth as u16,
                height: icon_bitmap.bmHeight as u16,
                encoding: protocol::Encoding::Cursor,
            };
            message.write_to(&mut self.tcp_stream)?;
            rect.write_to(&mut self.tcp_stream)?;
            self.tcp_stream.write_all(&cursor_pixels)?;
            self.tcp_stream.write_all(&mask_pixels)?;
            self.tcp_stream.flush()?;
        };
        Ok(())
    }
}

impl ServerState {
    pub fn new() -> Self {
        ServerState {
            frame: AtomicUsize::new(0),
            connection_state: AtomicI32::new(ConnectionState::Init as i32),
            cursor_sent: AtomicIsize::new(-1),
        }
    }

    pub fn get_frame(&self) -> usize {
        self.frame.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn get_ready(&self) -> bool {
        self.connection_state.load(std::sync::atomic::Ordering::Relaxed) == ConnectionState::Ready as i32
    }

    pub fn get_terminating(&self) -> bool {
        self.connection_state.load(std::sync::atomic::Ordering::Relaxed) == ConnectionState::Terminating as i32
    }

    pub fn set_ready(&self) {
        self.connection_state.store(ConnectionState::Ready as i32, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set_terminating(&self) {
        self.connection_state.store(ConnectionState::Terminating as i32, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn inc_frame(&self) {
        self.frame.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get_cursor_sent(&self) -> isize {
        self.cursor_sent.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_cursor_sent(&self, hcursor: isize) {
        self.cursor_sent.store(hcursor, std::sync::atomic::Ordering::Relaxed);
    }
}
