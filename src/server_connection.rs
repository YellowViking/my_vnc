use std::{mem};
use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicUsize};
use std::sync::RwLock;
use std::thread::sleep;

use anyhow::bail;
use log::{debug, error, info, trace, warn};
use rust_vnc::protocol::{ButtonMaskFlags, Message, S2C};
use rust_vnc::protocol;
use windows::Win32::Foundation::{BOOL, COLORREF, POINT};
use windows::Win32::Graphics::Gdi;
use windows::Win32::Graphics::Gdi::{BITMAP, GetBitmapBits, GetObjectW, GetSysColor};
use windows::Win32::UI::WindowsAndMessaging::{CURSORINFO, GetCursorInfo, GetCursorPos, GetIconInfo, ICONINFO};

use crate::dxgl::DisplayDuplWrapper;

pub struct ServerConnection<'a> {
    tcp_stream: &'a TcpStream,
    pic_data: Vec<u8>,
    server_state: &'a ServerState,
    display_dupl_wrapper: &'a mut DisplayDuplWrapper,
}

pub struct ServerState {
    frame: AtomicUsize,
    connection_state: AtomicI32,
    cursor_sent: AtomicIsize,
    last_pointer_input: RwLock<protocol::C2S>,
    last_key_input: RwLock<HashMap<u32, bool>>,
    last_clipboard: RwLock<String>,
}

pub enum ConnectionState {
    Init = -1,
    Ready = 0,
    Terminating = 1,
}

impl<'a> ServerConnection<'a> {
    pub fn new(tcp_stream: &'a TcpStream, server_state: &'a ServerState, display_dupl_wrapper: &'a mut DisplayDuplWrapper) -> Self {
        let pic_data: Vec<u8> = vec![0; 0];
        ServerConnection {
            tcp_stream,
            pic_data,
            server_state,
            display_dupl_wrapper,
        }
    }

    pub(crate) fn update_frame_loop(&mut self) -> anyhow::Result<()> {
        let duration = std::time::Duration::from_millis(1000 / 10);
        info!("update_frame loop started");
        loop {
            if self.server_state.get_terminating() {
                info!("terminating update_frame loop");
                return Ok(());
            }
            let start = std::time::Instant::now();
            if self.server_state.get_ready() {
                let result = self.send_cursor();
                if let Err(e) = result {
                    warn!("Failed to send cursor: {:?}", e);
                }
                self.send_clipboard().unwrap_or_else(|e| warn!("Failed to send clipboard: {:?}", e));
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

    fn send_clipboard(&mut self) -> anyhow::Result<()> {
        let text = clipboard_win::get_clipboard_string().map_err(|e| anyhow::anyhow!(e))?;
        let mut guard = self.server_state.last_clipboard.write().unwrap();
        if *guard == text {
            return Ok(());
        }
        let message = S2C::CutText(text.clone());
        message.write_to(&mut self.tcp_stream)?;
        self.tcp_stream.flush()?;
        *guard = text;
        Ok(())
    }

    fn acquire_frame(&mut self) -> anyhow::Result<()> {
        // draw frame count on the hdc
        self.display_dupl_wrapper.draw_to_texture(|hdc| -> anyhow::Result<()> {
            let frame = self.server_state.frame.load(std::sync::atomic::Ordering::Relaxed);

            let mut cursor_pos = POINT::default();
            unsafe {
                if let Err(e) = GetCursorPos(&mut cursor_pos) {
                    error!("GetCursorPos failed with error: {:?}", e);
                }
            }

            let text = format!("Frame: {}, Pos: ({}, {})", frame, cursor_pos.x, cursor_pos.y);
            unsafe {
                Gdi::SetBkMode(hdc, Gdi::TRANSPARENT);
                let sys_color = GetSysColor(Gdi::COLOR_HIGHLIGHTTEXT);
                Gdi::SetTextColor(hdc, COLORREF(sys_color));
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
        let expected_pic_data_len = self.display_dupl_wrapper.get_dimensions()?.0 as usize * self.display_dupl_wrapper.get_dimensions()?.1 as usize * 4;
        if self.pic_data.len() != expected_pic_data_len {
            bail!("pic_data length mismatch: expected: {}, actual: {}", expected_pic_data_len, self.pic_data.len());
        }
        Ok(())
    }

    fn send_frame(&mut self) -> anyhow::Result<()> {
        debug!("frame acquired: {} bytes dimensions: {:?}", self.pic_data.len(), self.display_dupl_wrapper.get_dimensions()?);
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
            let icon_bitmap = BITMAP::default();
            GetObjectW(hbmp, std::mem::size_of::<BITMAP>() as i32, Some(&icon_bitmap as *const _ as *mut c_void));
            let icon_bitmap_mask = BITMAP::default();
            GetObjectW(icon_info.hbmMask, std::mem::size_of::<BITMAP>() as i32, Some(&icon_bitmap_mask as *const _ as *mut c_void));

            info!("icon_info: {:?}, cursor_info: {:?}", icon_info, cursor_info);
            info!("icon_bitmap: {:?}", icon_bitmap);
            info!("icon_bitmap_mask: {:?}", icon_bitmap_mask);
            let mut cursor_pixels = vec![0; (icon_bitmap.bmWidthBytes * icon_bitmap.bmHeight) as usize];
            let bytes = GetBitmapBits(hbmp, cursor_pixels.len() as i32, cursor_pixels.as_mut_ptr() as *mut c_void);
            info!("icon_bitmap copied: {}", bytes);
            std::fs::File::create("icon.bin")?.write_all(&cursor_pixels)?;
            let mut mask_pixels = vec![0; (icon_bitmap_mask.bmWidthBytes * icon_bitmap_mask.bmHeight) as usize];
            let bytes = GetBitmapBits(icon_info.hbmMask, mask_pixels.len() as i32, mask_pixels.as_mut_ptr() as *mut c_void);
            if hbmp.is_invalid() {
                bail!("Failed to get cursor bitmap");
            }
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
            trace!("sending cursor: {:?}", rect);
            message.write_to(&mut self.tcp_stream)?;
            rect.write_to(&mut self.tcp_stream)?;
            trace!("sending cursor pixels: {} bytes, mask pixels: {} bytes", cursor_pixels.len(), mask_pixels.len());
            self.tcp_stream.write_all(&cursor_pixels)?;
            self.tcp_stream.write_all(&mask_pixels)?;
            trace!("cursor sent");
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
            last_pointer_input: RwLock::new(protocol::C2S::PointerEvent {
                x_position: 0,
                y_position: 0,
                button_mask: ButtonMaskFlags::empty(),
            }),
            last_key_input: RwLock::new(HashMap::new()),
            last_clipboard: RwLock::new(String::new()),
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

    pub fn set_last_pointer_input(&self, input: protocol::C2S) {
        *self.last_pointer_input.write().unwrap() = input;
    }

    pub fn get_last_pointer_input<T>(&self, cb: impl FnOnce(&protocol::C2S) -> T) -> T {
        cb(&*self.last_pointer_input.read().unwrap())
    }

    pub fn set_last_key_input(&self, key: u32, down: bool) {
        self.last_key_input.write().unwrap().insert(key, down);
    }

    pub fn get_last_key_input(&self, key: u32) -> bool {
        self.last_key_input.read().unwrap().get(&key).copied().unwrap_or(false)
    }
}
