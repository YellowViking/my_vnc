use std::collections::HashMap;
use std::ops::DerefMut;
use std::sync::{Arc, Mutex, RwLock};

use lazy_static::lazy_static;
use log::{debug, error, info};
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Device4, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::IDXGISurface1;
use windows::Win32::Graphics::Gdi::HDC;

use win_desktop_duplication::{co_init, DesktopDuplicationApi, DuplicationApiOptions, set_process_dpi_awareness};
use win_desktop_duplication::devices::AdapterFactory;
use win_desktop_duplication::outputs::Display;
use win_desktop_duplication::tex_reader::TextureReader;
use win_desktop_duplication::texture::Texture;

pub struct DisplayDuplWrapper {
    display: u16,
    id3d11texture2d: ID3D11Texture2D,
}

impl DisplayDuplWrapper {
    pub fn get_dimensions(&self) -> anyhow::Result<(u16, u16)> {
        get_display_dimensions(self.display)
    }

    pub fn new(display: u16) -> anyhow::Result<Self> {
        get_display_dupl(display, |display_dupl| -> anyhow::Result<Self> {
            unsafe {
                let dev: ID3D11Device4 = display_dupl.dupl.get_device_and_ctx().0;
                let mut d3d_tex_desc: D3D11_TEXTURE2D_DESC = Default::default();
                let src_texture = display_dupl.get_raw_texture()?;
                src_texture.GetDesc(&mut d3d_tex_desc);
                info!("d3d_tex_desc: {:?}", d3d_tex_desc);
                let mut id3d11texture2d = None;
                dev.CreateTexture2D(&d3d_tex_desc, None, Some(&mut id3d11texture2d))?; // clone texture
                if let None = id3d11texture2d {
                    anyhow::bail!("Failed to create texture");
                }
                Ok(DisplayDuplWrapper {
                    display,
                    id3d11texture2d: id3d11texture2d.unwrap(),
                })
            }
        })
    }

    pub fn copy_from_desktop(&mut self) -> anyhow::Result<()> {
        get_display_dupl(self.display, |display_dupl| {
            unsafe {
                let dev_ctx = display_dupl.dupl.get_device_and_ctx().1;
                let src_texture = display_dupl.get_raw_texture()?;
                dev_ctx.CopyResource(&self.id3d11texture2d, src_texture);
                dev_ctx.Flush();
                debug!("copied from desktop {:?} to {:?}", src_texture, &self.id3d11texture2d);
                Ok(())
            }
        })
    }

    pub fn draw_to_texture(&self, draw_action: impl Fn(HDC) -> anyhow::Result<()>) -> anyhow::Result<()> {
        unsafe {
            let surface: IDXGISurface1 = self.id3d11texture2d.cast()?;
            let hdc: HDC;
            hdc = surface.GetDC(false)?;
            draw_action(hdc)?;
            surface.ReleaseDC(None)?;
            Ok(())
        }
    }

    pub fn copy_to_vec(&self) -> anyhow::Result<Vec<u8>> {
        get_display_dupl(self.display, |display_dupl| -> anyhow::Result<Vec<u8>> {
            let mut vec = Vec::new();
            let (dev, ctx) = display_dupl.dupl.get_device_and_ctx();
            let mut tex_reader = TextureReader::new(dev, ctx);
            tex_reader.get_data(&mut vec, &Texture::new(self.id3d11texture2d.clone()))?;
            Ok(vec)
        })
    }
}

struct DisplayDupl {
    display_output: Display,
    dupl: DesktopDuplicationApi,
    texture: Option<Texture>,
}

impl DisplayDupl {
    pub fn get_raw_texture(&self) -> anyhow::Result<&ID3D11Texture2D> {
        match &self.texture {
            None => { anyhow::bail!("No texture available") }
            Some(tex) => {
                Ok(tex.as_raw_ref())
            }
        }
    }
}

fn get_display_dimensions(display: u16) -> anyhow::Result<(u16, u16)> {
    get_display_dupl(display, |display_dupl|
    {
        let mode = display_dupl.display_output.get_current_display_mode()?;
        Ok((mode.width as u16, mode.height as u16))
    })
}

fn get_display_dupl<T>(display: u16, action: impl Fn(&mut DisplayDupl) -> anyhow::Result<T>) -> anyhow::Result<T>
{
    {
        let mut guard = DISPLAY_MAP.lock().unwrap();
        let entry = guard.entry(display);
        let display_dupl = entry.or_insert_with(|| {
            let dupl = init_dxgl_inner(display);
            let arc = Arc::new(RwLock::new(dupl));
            let arc_clone = arc.clone();
            std::thread::spawn(move || {
                info!("frame_reader_thread started for display {}", display);
                display_duplicate_loop(arc_clone);
            });
            return arc;
        });
        let result = display_dupl.write();
        match result {
            Err(e) => {
                anyhow::bail!("Error in frame_reader_thread: {:?}", e)
            }
            Ok(mut display_dupl) => {
                return action(display_dupl.deref_mut());
            }
        }
    }
}

fn display_duplicate_loop(arc_clone: Arc<RwLock<DisplayDupl>>) {
    const FRAME_REFRESH: core::time::Duration = core::time::Duration::from_millis(1000 / 10);
    loop {
        let start_time = std::time::Instant::now();
        {
            let display_dupl = arc_clone.write();
            match display_dupl {
                Ok(mut display_dupl) => {
                    let display_dupl = display_dupl.deref_mut();
                    if let Err(e) = process_frame(display_dupl) {
                        error!("Error in frame_reader_thread: {:?}", e);
                    }
                }
                Err(e) => {
                    error!("Error in frame_reader_thread: {:?}", e);
                }
            }
        }
        let elapsed = start_time.elapsed();
        if elapsed < FRAME_REFRESH {
            std::thread::sleep(FRAME_REFRESH - elapsed);
        }
    };
}

fn process_frame(display_dupl: &mut DisplayDupl) -> anyhow::Result<()> {
    display_dupl.display_output.wait_for_vsync()?;
    let tex = display_dupl.dupl.acquire_next_frame_now();
    display_dupl.texture = Some(tex?);
    Ok(())
}


fn init_dxgl_inner(display: u16) -> DisplayDupl {
    info!("init_dxgl for display {}", display);

    // this is required to be able to use desktop duplication api
    set_process_dpi_awareness();
    co_init();

    // select gpu and output you want to use.
    let adapter = AdapterFactory::new().get_adapter_by_idx(0).unwrap();
    let display_output = adapter.get_display_by_idx(display as u32).unwrap();

    // get output duplication api
    let mut dupl = DesktopDuplicationApi::new(adapter, display_output.clone()).unwrap();
    dupl.configure(DuplicationApiOptions {
        skip_cursor: true,
    });
    let texture = dupl.acquire_next_frame_now().unwrap();
    info!("texture: {:?}", texture.desc());

    DisplayDupl {
        display_output,
        dupl,
        texture: Some(texture),
    }
}

lazy_static! {
    static ref DISPLAY_MAP: Mutex<HashMap<u16, Arc<RwLock<DisplayDupl>>>> = Mutex::new(HashMap::new());
}
