use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Foundation;
pub trait DisplayDuplicator {
    fn get_dimensions(&self) -> anyhow::Result<(u16, u16)>;
    fn new(display: u16) -> anyhow::Result<Self> where Self: Sized;
    fn copy_from_desktop(&mut self) -> anyhow::Result<()>;
    fn draw_to_texture(
        &mut self,
        draw_action: impl Fn(HDC) -> anyhow::Result<Foundation::RECT>,
    ) -> anyhow::Result<()>;
    fn copy_to_vec(&self) -> anyhow::Result<Vec<u8>>;
    fn get_dirty_rects(&self) -> &Vec<Foundation::RECT>;
}