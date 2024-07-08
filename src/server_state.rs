use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicUsize};
use std::sync::RwLock;
use rust_vnc::protocol;
use rust_vnc::protocol::{ButtonMaskFlags, Encoding};
use std::collections::HashMap;
use windows::Win32::Foundation;

pub enum ConnectionState {
    Init = -1,
    Ready = 0,
    Terminating = 1,
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
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
            bytes_send: AtomicUsize::new(0),
            frame_encoding: AtomicI32::new(-1),
            last_stats_size: RwLock::new(Foundation::SIZE::default()),
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
        cb(&self.last_pointer_input.read().unwrap())
    }

    pub fn set_last_key_input(&self, key: u32, down: bool) {
        self.last_key_input.write().unwrap().insert(key, down);
    }

    pub fn get_last_key_input(&self, key: u32) -> bool {
        self.last_key_input.read().unwrap().get(&key).copied().unwrap_or(false)
    }

    pub fn add_bytes_send(&self, bytes: usize) {
        self.bytes_send.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get_bytes_send(&self) -> usize {
        self.bytes_send.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn get_and_set_last_clipboard(&self, cb: impl FnOnce(&str) -> anyhow::Result<String>) -> anyhow::Result<()> {
        let mut guard = self.last_clipboard.write().unwrap();
        let val = cb(guard.as_str());
        let ret = match val {
            Ok(_) => {
                Ok(())
            }
            Err(e) => {
                anyhow::bail!(e)
            }
        };
        if let Ok(val) = val {
            *guard = val;
        }
        ret
    }

    pub fn get_frame_encoding(&self) -> Encoding {
        self.frame_encoding.load(std::sync::atomic::Ordering::Relaxed).into()
    }

    pub fn set_frame_encoding(&self, encoding: Encoding) {
        self.frame_encoding.store(encoding.into(), std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get_last_stats_size(&self) -> Foundation::SIZE {
        *self.last_stats_size.read().unwrap()
    }

    pub fn set_last_stats_size(&self, size: Foundation::SIZE) {
        *self.last_stats_size.write().unwrap() = size;
    }
}

pub struct ServerState {
    frame: AtomicUsize,
    connection_state: AtomicI32,
    cursor_sent: AtomicIsize,
    last_pointer_input: RwLock<protocol::C2S>,
    last_key_input: RwLock<HashMap<u32, bool>>,
    last_clipboard: RwLock<String>,
    bytes_send: AtomicUsize,
    frame_encoding: AtomicI32,
    last_stats_size: RwLock<Foundation::SIZE>,
}
