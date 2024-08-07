use std::ffi::c_char;
use crate::server::Args;
use crate::settings::init_logger;
use tracing::{error};
use windows::core::PCSTR;
use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxA, MB_OK};

// File: my_vnc
pub mod dxgl;
mod gdi;
pub mod network_stream;
pub mod server;
pub mod server_connection;
pub mod server_events;
pub mod server_state;
pub mod settings;
mod traits;

#[no_mangle]
#[allow(non_snake_case, unused_variables)]
extern "system" fn DllMain(dll_module: HINSTANCE, call_reason: u32, _: *mut ()) -> bool {
    match call_reason {
        DLL_PROCESS_ATTACH => unsafe {
            // write pic to file
            let pid = GetCurrentProcessId();
            let pid = pid.to_string();
            std::fs::write("c:/shared/pid.txt", pid).unwrap();
        },
        DLL_PROCESS_DETACH => unsafe {
            MessageBoxA(
                None,
                PCSTR("Goodbye, World!".as_ptr()),
                PCSTR("world".as_ptr()),
                MB_OK,
            );
        },
        _ => (),
    }

    true
}
#[no_mangle]
pub extern "C" fn PrintUIEntry(
    msg: *const c_char,
) {
    let msg = unsafe { std::ffi::CStr::from_ptr(msg) };
    let mut host = msg.to_str().unwrap_or("fox-pc");
    let vec = host.split(":").collect::<Vec<&str>>();
    let mut port = 80;
    if vec.len() > 1 {
        host = vec[0];
        port = vec[1].parse::<u16>().unwrap_or(80);
    }
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                init_logger();
                server::main_args(
                    Args {
                        use_tunnelling: true,
                        host: host.to_string(),
                        port,
                        display: 0,
                        use_gdi: true,
                        enable_profiling: true,
                    },
                ).await;
                unsafe {
                    MessageBoxA(
                        None,
                        PCSTR("Server terminated".as_ptr()),
                        PCSTR("world".as_ptr()),
                        MB_OK,
                    );
                }
                error!("server terminated");
            });
    });
    unsafe {
        MessageBoxA(
            None,
            PCSTR("Server started".as_ptr()),
            PCSTR("world".as_ptr()),
            MB_OK,
        );
    }
}
