use std::cmp::max;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::mem;
use std::mem::size_of;
use std::thread::sleep;

use anyhow::bail;
use bytesize::ByteSize;
use flate2::write::ZlibEncoder;
use rust_vnc::protocol;
use rust_vnc::protocol::{Message, S2C};
use tracing::{debug, error, info, info_span, trace, warn};
use windows::Win32::Foundation;
use windows::Win32::Foundation::{BOOL, COLORREF, POINT, SIZE};
use windows::Win32::Graphics::Gdi;
use windows::Win32::Graphics::Gdi::{
    GetBitmapBits, GetObjectW, GetSysColor, GetTextExtentPointA, BITMAP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, GetCursorPos, GetIconInfo, CURSORINFO, ICONINFO,
};

use crate::network_stream::CloneableStream;
use crate::server_state::ServerState;
use crate::traits::DisplayDuplicator;

pub struct ServerConnection<'a, DisplayDupl>
where
    DisplayDupl: DisplayDuplicator,
{
    tcp_stream: MonitoredTcpStream<'a>,
    pic_data: Vec<u8>,
    server_state: &'a ServerState,
    display_dupl_wrapper: &'a mut DisplayDupl,
    zlib_encoder: ZlibEncoder<VecDeque<u8>>,
}

struct MonitoredTcpStream<'a> {
    tcp_stream: CloneableStream,
    server_state: &'a ServerState,
}

impl<'a> Write for MonitoredTcpStream<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let result = self.tcp_stream.write(buf);
        if result.is_ok() {
            self.server_state.add_bytes_send(buf.len());
        }
        result
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.tcp_stream.flush()
    }
}

impl<'a> MonitoredTcpStream<'a> {
    fn new(tcp_stream: CloneableStream, server_state: &'a ServerState) -> Self {
        MonitoredTcpStream {
            tcp_stream,
            server_state,
        }
    }
}

impl<'a, DisplayDupl> ServerConnection<'a, DisplayDupl>
where
    DisplayDupl: DisplayDuplicator,
{
    pub fn new(
        tcp_stream: CloneableStream,
        server_state: &'a ServerState,
        display_dupl_wrapper: &'a mut DisplayDupl,
    ) -> Self {
        let pic_data: Vec<u8> = vec![0; 0];
        let tcp_stream = MonitoredTcpStream::new(tcp_stream, server_state);
        ServerConnection {
            tcp_stream,
            pic_data,
            server_state,
            display_dupl_wrapper,
            zlib_encoder: ZlibEncoder::new(
                VecDeque::new(),
                flate2::Compression::best(),
            ),
        }
    }

    pub fn update_frame_loop(&mut self) -> anyhow::Result<()> {
        let duration = std::time::Duration::from_millis(1000 / 10);
        info!("update_frame loop started");
        loop {
            puffin::profile_function!();
            let span = info_span!("update_frame");
            let _guard = span.enter();
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
                self.send_clipboard()
                    .unwrap_or_else(|e| warn!("Failed to send clipboard: {:?}", e));
                self.display_dupl_wrapper.copy_from_desktop()?;
                self.acquire_frame()?;
                self.send_frame()?;
            }
            let elapsed = start.elapsed();
            if duration > elapsed {
                sleep(duration - elapsed);
            }
            self.server_state.inc_frame();
            puffin::GlobalProfiler::lock().new_frame();
        }
    }

    fn send_clipboard(&mut self) -> anyhow::Result<()> {
        let text = clipboard_win::get_clipboard_string().map_err(|e| anyhow::anyhow!(e))?;
        self.server_state.get_and_set_last_clipboard(|last| {
            if last == text {
                return Ok(text);
            }
            let message = S2C::CutText(text.clone());
            message.write_to(&mut self.tcp_stream)?;
            self.tcp_stream.flush()?;
            Ok(text)
        })
    }

    fn acquire_frame(&mut self) -> anyhow::Result<()> {
        puffin::profile_function!();
        // draw frame count on the hdc
        self.display_dupl_wrapper
            .draw_to_texture(|hdc| -> anyhow::Result<Foundation::RECT> {
                let frame = self.server_state.get_frame();

                let mut cursor_pos = POINT::default();
                unsafe {
                    if let Err(e) = GetCursorPos(&mut cursor_pos) {
                        error!("GetCursorPos failed with error: {:?}", e);
                    }
                }
                let bytes = ByteSize::b(self.server_state.get_bytes_send() as u64);
                let text = format!(
                    "Frame: {}, Pos: ({}, {}) Bytes: {}",
                    frame, cursor_pos.x, cursor_pos.y, bytes
                );
                let mut text_size = SIZE::default();
                let dirty_rect;
                unsafe {
                    let bool = GetTextExtentPointA(hdc, text.as_str().as_ref(), &mut text_size);
                    if !bool.as_bool() {
                        bail!(
                            "Failed to get text size, error: {:?}",
                            windows::Win32::Foundation::GetLastError()
                        )
                    }
                    let mut size = self.server_state.get_last_stats_size();
                    self.server_state.set_last_stats_size(text_size);
                    size.cx = max(size.cx, text_size.cx);
                    size.cy = max(size.cy, text_size.cy);
                    dirty_rect = Foundation::RECT {
                        left: 0,
                        top: 0,
                        right: size.cx,
                        bottom: size.cy,
                    };
                    Gdi::SetBkMode(hdc, Gdi::TRANSPARENT);
                    let sys_color = GetSysColor(Gdi::COLOR_HIGHLIGHTTEXT);
                    Gdi::SetTextColor(hdc, COLORREF(sys_color));
                    match Gdi::TextOutA(hdc, 0, 0, text.as_str().as_ref()) {
                        BOOL(b) => {
                            if b == 0 {
                                bail!(
                                    "Failed to draw text, error: {:?}",
                                    windows::Win32::Foundation::GetLastError()
                                )
                            }
                        }
                    }
                };
                Ok(dirty_rect)
            })?;

        let result = self.display_dupl_wrapper.copy_to_vec();
        if let Err(e) = result {
            bail!("Failed to read texture data: {:?}", e);
        }

        self.pic_data = result?;
        let expected_pic_data_len = self.display_dupl_wrapper.get_dimensions()?.0 as usize
            * self.display_dupl_wrapper.get_dimensions()?.1 as usize
            * 4;
        if self.pic_data.len() != expected_pic_data_len {
            bail!(
                "pic_data length mismatch: expected: {}, actual: {}",
                expected_pic_data_len,
                self.pic_data.len()
            );
        }
        Ok(())
    }

    fn send_frame(&mut self) -> anyhow::Result<()> {
        puffin::profile_function!();
        let pixel_byte_size = 4i32;
        debug!(
            "frame acquired: {} bytes dimensions: {:?}",
            self.pic_data.len(),
            self.display_dupl_wrapper.get_dimensions()?
        );
        let mut rects = { self.display_dupl_wrapper.get_dirty_rects() };
        trace!("sending {} rects", rects.len());
        let full_rect = vec![Foundation::RECT {
            left: 0,
            top: 0,
            right: self.display_dupl_wrapper.get_dimensions()?.0 as i32,
            bottom: self.display_dupl_wrapper.get_dimensions()?.1 as i32,
        }];
        if self.server_state.get_frame() < 2 {
            info!("sending full frame {:?}", full_rect);
            rects = &full_rect;
        }
        let message = S2C::FramebufferUpdate {
            count: rects.len() as u16,
        };
        message.write_to(&mut self.tcp_stream)?;
        let line_size = self.display_dupl_wrapper.get_dimensions()?.0 as i32 * pixel_byte_size;
        for rect in rects {
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            let mut pixel_buf = Vec::with_capacity(
                (width * height * pixel_byte_size) as usize + size_of::<protocol::Rectangle>(),
            );
            let vnc_rect = protocol::Rectangle {
                x_position: rect.left as u16,
                y_position: rect.top as u16,
                width: width as u16,
                height: height as u16,
                encoding: protocol::Encoding::Unknown(-1),
            };
            for line in 0..height {
                let start = (rect.top + line) * line_size + rect.left * pixel_byte_size;
                let end = start + width * pixel_byte_size;
                pixel_buf.write_all(&self.pic_data[start as usize..end as usize])?
            }
            pixel_buf.flush()?;
            let encoder = &mut self.zlib_encoder;
            let buf = Self::encode_rect(&self.server_state, vnc_rect, pixel_buf, encoder)?;
            self.tcp_stream
                .write_all(&buf)?;
        }
        self.tcp_stream.flush()?;
        trace!("frame sent for rects: {:?}", rects.len());
        Ok(())
    }

    fn send_cursor(&mut self) -> anyhow::Result<()> {
        unsafe {
            let mut cursor_info = CURSORINFO::default();
            cursor_info.cbSize = mem::size_of::<CURSORINFO>() as u32;
            GetCursorInfo(&mut cursor_info)?;
            let hcursor = cursor_info.hCursor;
            if self.server_state.get_cursor_sent() == hcursor.0 as isize {
                return Ok(());
            }
            self.server_state.set_cursor_sent(hcursor.0 as isize);
            let mut icon_info = ICONINFO::default();
            GetIconInfo(hcursor, &mut icon_info)?;
            let hbmp = icon_info.hbmColor;
            let icon_bitmap = BITMAP::default();
            GetObjectW(
                hbmp,
                std::mem::size_of::<BITMAP>() as i32,
                Some(&icon_bitmap as *const _ as *mut c_void),
            );
            let icon_bitmap_mask = BITMAP::default();
            GetObjectW(
                icon_info.hbmMask,
                std::mem::size_of::<BITMAP>() as i32,
                Some(&icon_bitmap_mask as *const _ as *mut c_void),
            );

            info!("icon_info: {:?}, cursor_info: {:?}", icon_info, cursor_info);
            info!("icon_bitmap: {:?}", icon_bitmap);
            info!("icon_bitmap_mask: {:?}", icon_bitmap_mask);
            let mut cursor_pixels =
                vec![0; (icon_bitmap.bmWidthBytes * icon_bitmap.bmHeight) as usize];
            let bytes = GetBitmapBits(
                hbmp,
                cursor_pixels.len() as i32,
                cursor_pixels.as_mut_ptr() as *mut c_void,
            );
            info!("icon_bitmap copied: {}", bytes);
            std::fs::File::create("icon.bin")?.write_all(&cursor_pixels)?;
            let mut mask_pixels =
                vec![0; (icon_bitmap_mask.bmWidthBytes * icon_bitmap_mask.bmHeight) as usize];
            let bytes = GetBitmapBits(
                icon_info.hbmMask,
                mask_pixels.len() as i32,
                mask_pixels.as_mut_ptr() as *mut c_void,
            );
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

            let message = S2C::FramebufferUpdate { count: 1 };
            let rect = protocol::Rectangle {
                x_position: 0,
                y_position: 0,
                width: icon_bitmap.bmWidth as u16,
                height: icon_bitmap.bmHeight as u16,
                encoding: protocol::Encoding::Known(protocol::KnownEncoding::Cursor),
            };
            trace!("sending cursor: {:?}", rect);
            message.write_to(&mut self.tcp_stream)?;
            rect.write_to(&mut self.tcp_stream)?;
            trace!(
                "sending cursor pixels: {} bytes, mask pixels: {} bytes",
                cursor_pixels.len(),
                mask_pixels.len()
            );
            self.tcp_stream.write_all(&cursor_pixels)?;
            self.tcp_stream.write_all(&mask_pixels)?;
            trace!("cursor sent");
            self.tcp_stream.flush()?;
        };
        Ok(())
    }
    fn encode_rect<T>(server_state: &ServerState, mut rect: protocol::Rectangle, buf: Vec<u8>, encoder: &mut ZlibEncoder<T>) -> anyhow::Result<Vec<u8>>
    where
        T: Write + Read,
    {
        let buf_len = buf.len();
        let mut ret = Vec::with_capacity(buf_len);
        if server_state.get_frame_encoding()
            == protocol::Encoding::Known(protocol::KnownEncoding::Zlib)
        {
            let out = encoder.total_out() as usize;
            encoder.write_all(&buf)?;
            encoder.flush()?;
            let mut compressed = Vec::with_capacity(encoder.total_out() as usize - out);
            encoder.read_to_end(&mut compressed)?;

            rect.encoding = protocol::Encoding::Known(protocol::KnownEncoding::Zlib);
            rect.write_to(&mut ret)?;
            compressed.write_to(&mut ret)?;
            trace!(
                "compressed: {} bytes, uncompressed: {} bytes",
                ret.len(),
                buf_len + size_of::<protocol::Rectangle>()
            );
        } else {
            rect.encoding = protocol::Encoding::Known(protocol::KnownEncoding::Raw);
            rect.write_to(&mut ret)?;
            ret.write_all(&buf)?;
        }
        Ok(ret)
    }
}
