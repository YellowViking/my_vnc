use log::trace;
use tracing::{info, instrument};
use crate::traits::DisplayDuplicator;
use windows::Win32::Foundation;
use windows::Win32::Graphics::Gdi::{BITMAPINFO, GetDC, HBITMAP, HDC};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

struct MyHdc(HDC);
struct MyHbitmap(HBITMAP);
unsafe impl Send for MyHdc {}
unsafe impl Send for MyHbitmap {}
pub(crate) struct GdiDisplayDuplicator {
    #[warn(dead_code)]
    display: u16,
    dirty_rects: Vec<Foundation::RECT>,
    hdc_bitmap: MyHdc,
    hdc_screen: MyHdc,
    hbitmap: MyHbitmap,
    vec: Vec<u8>,
}

impl Drop for GdiDisplayDuplicator {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::DeleteDC(self.hdc_bitmap.0);
            let _ = windows::Win32::Graphics::Gdi::DeleteDC(self.hdc_screen.0);
        }
    }
}
impl DisplayDuplicator for GdiDisplayDuplicator {
    fn get_dimensions(&self) -> anyhow::Result<(u16, u16)> {
        // Implementation to get display dimensions
        let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        Ok((width as u16, height as u16))
    }

    fn new(display: u16) -> anyhow::Result<Self> {
        // Initialize GDI and create a new instance
        let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        let buf_size = width as usize * height as usize * 4;

        let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow() };
        let hdc_screen = unsafe { GetDC(hwnd) };
        if hdc_screen.0 == std::ptr::null_mut() {
            return Err(std::io::Error::last_os_error().into());
        }
        let hdc_target = unsafe { windows::Win32::Graphics::Gdi::CreateCompatibleDC(hdc_screen) };
        if hdc_target.0 == std::ptr::null_mut() {
            return Err(std::io::Error::last_os_error().into());
        }

        let rect = Foundation::RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        let hbitmap = unsafe {
            windows::Win32::Graphics::Gdi::CreateCompatibleBitmap(
                hdc_screen,
                rect.right,
                rect.bottom,
            )
        };
        if hbitmap.0 == std::ptr::null_mut() {
            return Err(std::io::Error::last_os_error().into());
        }
        let old_obj = unsafe { windows::Win32::Graphics::Gdi::SelectObject(hdc_target, hbitmap) };
        if old_obj.0 == std::ptr::null_mut() {
            return Err(std::io::Error::last_os_error().into());
        }
        info!("buf_size: {}", buf_size);
        Ok(GdiDisplayDuplicator {
            display,
            dirty_rects: Vec::new(),
            vec: vec![0u8; buf_size],
            hdc_bitmap: MyHdc(hdc_target),
            hdc_screen: MyHdc(hdc_screen),
            hbitmap: MyHbitmap(hbitmap),
        })
    }

    #[instrument(level = "trace", ret, skip(self))]
    fn copy_from_desktop(&mut self) -> anyhow::Result<()> {
        puffin::profile_function!();
        unsafe {
            let (width, height) = self.get_dimensions()?;
            let hdc_target = self.hdc_bitmap.0;
            let hdc_screen = self.hdc_screen.0;
            windows::Win32::Graphics::Gdi::BitBlt(
                hdc_target,
                0,
                0,
                width as i32,
                height as i32,
                hdc_screen,
                0,
                0,
                windows::Win32::Graphics::Gdi::SRCCOPY,
            )?;
        }
        Ok(())
    }

    fn draw_to_texture(
        &mut self,
        draw_action: impl Fn(HDC) -> anyhow::Result<Foundation::RECT>,
    ) -> anyhow::Result<()> {
        puffin::profile_function!();
        draw_action(self.hdc_bitmap.0)?;
        let mut vec = self.copy_desktop_to_buf()?;
        // compute dirty rects between vec and self.vec line by line
        self.update_dirty_rects(&mut vec)?;
        self.vec = vec;
        Ok(())
    }

    fn copy_to_vec(&self) -> anyhow::Result<Vec<u8>> {
        Ok(self.vec.clone())
    }

    fn get_dirty_rects(&self) -> &Vec<Foundation::RECT> {
        &self.dirty_rects
    }
}

impl GdiDisplayDuplicator {
    fn get_buf_size(rect: (u16, u16)) -> usize {
        rect.0 as usize * rect.1 as usize * 4
    }

    fn get_bitmap_info(width: u16, height: u16) -> BITMAPINFO {
        windows::Win32::Graphics::Gdi::BITMAPINFO {
            bmiHeader: windows::Win32::Graphics::Gdi::BITMAPINFOHEADER {
                biSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>()
                    as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: windows::Win32::Graphics::Gdi::BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [windows::Win32::Graphics::Gdi::RGBQUAD::default(); 1],
        }
    }

    fn update_dirty_rects(&mut self, vec: &mut Vec<u8>) -> anyhow::Result<()> {
        let (width, height) = self.get_dimensions()?;
        self.dirty_rects.clear();
        for line_num in 0..height as i32 {
            let start = line_num as usize * width as usize * 4;
            let end = start + width as usize * 4;
            let line = &vec[start..end];
            let old_line = &self.vec[start..end];
            if line != old_line {
                self.dirty_rects.push(Foundation::RECT {
                    left: 0,
                    top: line_num,
                    right: width as i32,
                    bottom: line_num + 1,
                });
            }
        }
        Ok(())
    }

    fn copy_desktop_to_buf(&mut self) -> anyhow::Result<Vec<u8>> {
        puffin::profile_function!();
        let (width, height) = self.get_dimensions()?;
        let hdc_target = self.hdc_bitmap.0;

        // copy the bitmap to the vec buffer
        let mut bitmap_info = Self::get_bitmap_info(width, height);

        let buf_size = Self::get_buf_size((width, height));
        trace!("buf_size: {}", buf_size);
        let mut vec: Vec<u8> = Vec::with_capacity(buf_size);
        let result = unsafe {
            windows::Win32::Graphics::Gdi::GetDIBits(
                hdc_target,
                self.hbitmap.0,
                0,
                height as u32,
                Some(vec.as_mut_ptr() as *mut std::ffi::c_void),
                &mut bitmap_info,
                windows::Win32::Graphics::Gdi::DIB_RGB_COLORS,
            )
        };

        if result == 0 {
            return Err(std::io::Error::last_os_error().into());
        } else {
            unsafe { vec.set_len(buf_size); }
            trace!("GetDIBits result: {}", result);
        }
        Ok(vec)
    }
}
