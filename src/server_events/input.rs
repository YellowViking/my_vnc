use std::mem::size_of;
use std::ops::BitXor;

use clipboard_win::set_clipboard_string;
use rust_vnc::protocol::{ButtonMaskFlags, C2S};
use tracing::{error, info, trace, warn};
use win_key_codes;
use windows::Win32::Foundation::GetLastError;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, VkKeyScanA, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
};
use xkeysym;
use xkeysym::{key, Keysym};

use crate::server_state::ServerState;

pub fn handle_pointer_event(server_state: &ServerState, message: C2S) {
    if let C2S::PointerEvent {
        x_position,
        y_position,
        button_mask,
    } = message
    {
        let input = server_state.get_last_pointer_input(|last_input| -> Option<INPUT> {
            unsafe {
                if let C2S::PointerEvent {
                    button_mask: last_button_mask,
                    x_position: _last_x_position,
                    y_position: _last_y_position,
                } = last_input
                {
                    if *last_input == message {
                        return None;
                    }
                    let mut dw_flags = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
                    let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
                    let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
                    let dx = x_position as i32 * 65535 / (width - 1);
                    let dy = y_position as i32 * 65535 / (height - 1);
                    let mut mouse_data = 0i32;
                    if *last_button_mask == button_mask {
                        dw_flags |= MOUSEEVENTF_MOVE;
                    } else {
                        let last_button_mask = last_button_mask;
                        let xor_mask = last_button_mask.bitxor(button_mask);
                        if xor_mask.contains(ButtonMaskFlags::LEFT) {
                            if button_mask.contains(ButtonMaskFlags::LEFT) {
                                dw_flags |= MOUSEEVENTF_LEFTDOWN;
                            } else {
                                dw_flags |= MOUSEEVENTF_LEFTUP;
                            }
                        }

                        if xor_mask.contains(ButtonMaskFlags::MIDDLE) {
                            if button_mask.contains(ButtonMaskFlags::MIDDLE) {
                                dw_flags |= MOUSEEVENTF_MIDDLEDOWN;
                            } else {
                                dw_flags |= MOUSEEVENTF_MIDDLEUP;
                            }
                        }

                        if xor_mask.contains(ButtonMaskFlags::RIGHT) {
                            if button_mask.contains(ButtonMaskFlags::RIGHT) {
                                dw_flags |= MOUSEEVENTF_RIGHTDOWN;
                            } else {
                                dw_flags |= MOUSEEVENTF_RIGHTUP;
                            }
                        }

                        if button_mask.contains(ButtonMaskFlags::WHEEL_UP) {
                            dw_flags |= MOUSEEVENTF_WHEEL;
                            mouse_data = 120;
                        }
                        if button_mask.contains(ButtonMaskFlags::WHEEL_DOWN) {
                            dw_flags |= MOUSEEVENTF_WHEEL;
                            mouse_data = -120;
                        }
                    }

                    let input = INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 {
                            mi: MOUSEINPUT {
                                dx,
                                dy,
                                mouseData: mouse_data as u32,
                                dwFlags: dw_flags,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    };
                    return Some(input);
                }
            }
            None
        });

        server_state.set_last_pointer_input(message);

        unsafe {
            if let Some(input) = input {
                let input_array = [input];
                trace!("pointer event: {:?}", input.Anonymous.mi);
                let send_input = SendInput(&input_array, size_of::<INPUT>() as i32);
                if send_input == 0 {
                    let last_error = GetLastError();
                    error!("SendInput failed with error: {:?}", last_error);
                }
            } else {
                info!("pointer event: no input");
            }
        }
    }
}

fn map_xk_to_wvk(keysym: Keysym) -> VIRTUAL_KEY {
    let keysym = keysym.raw();
    match keysym {
        key::Shift_L => VIRTUAL_KEY(win_key_codes::VK_SHIFT as u16),
        key::Shift_R => VIRTUAL_KEY(win_key_codes::VK_SHIFT as u16),
        key::Control_L => VIRTUAL_KEY(win_key_codes::VK_CONTROL as u16),
        key::Control_R => VIRTUAL_KEY(win_key_codes::VK_CONTROL as u16),
        key::Alt_L => VIRTUAL_KEY(win_key_codes::VK_MENU as u16),
        key::Alt_R => VIRTUAL_KEY(win_key_codes::VK_MENU as u16),
        key::Super_L => VIRTUAL_KEY(win_key_codes::VK_LWIN as u16),
        key::Super_R => VIRTUAL_KEY(win_key_codes::VK_RWIN as u16),
        key::Caps_Lock => VIRTUAL_KEY(win_key_codes::VK_CAPITAL as u16),
        key::Num_Lock => VIRTUAL_KEY(win_key_codes::VK_NUMLOCK as u16),
        key::Scroll_Lock => VIRTUAL_KEY(win_key_codes::VK_SCROLL as u16),
        key::Page_Up => VIRTUAL_KEY(win_key_codes::VK_PRIOR as u16),
        key::Page_Down => VIRTUAL_KEY(win_key_codes::VK_NEXT as u16),
        key::Home => VIRTUAL_KEY(win_key_codes::VK_HOME as u16),
        key::End => VIRTUAL_KEY(win_key_codes::VK_END as u16),
        key::Insert => VIRTUAL_KEY(win_key_codes::VK_INSERT as u16),
        key::Delete => VIRTUAL_KEY(win_key_codes::VK_DELETE as u16),
        key::Left => VIRTUAL_KEY(win_key_codes::VK_LEFT as u16),
        key::Up => VIRTUAL_KEY(win_key_codes::VK_UP as u16),
        key::Right => VIRTUAL_KEY(win_key_codes::VK_RIGHT as u16),
        key::Down => VIRTUAL_KEY(win_key_codes::VK_DOWN as u16),
        key::F1 => VIRTUAL_KEY(win_key_codes::VK_F1 as u16),
        key::F2 => VIRTUAL_KEY(win_key_codes::VK_F2 as u16),
        key::F3 => VIRTUAL_KEY(win_key_codes::VK_F3 as u16),
        key::F4 => VIRTUAL_KEY(win_key_codes::VK_F4 as u16),
        key::F5 => VIRTUAL_KEY(win_key_codes::VK_F5 as u16),
        key::F6 => VIRTUAL_KEY(win_key_codes::VK_F6 as u16),
        key::F7 => VIRTUAL_KEY(win_key_codes::VK_F7 as u16),
        key::F8 => VIRTUAL_KEY(win_key_codes::VK_F8 as u16),
        key::F9 => VIRTUAL_KEY(win_key_codes::VK_F9 as u16),
        key::F10 => VIRTUAL_KEY(win_key_codes::VK_F10 as u16),
        key::F11 => VIRTUAL_KEY(win_key_codes::VK_F11 as u16),
        key::F12 => VIRTUAL_KEY(win_key_codes::VK_F12 as u16),
        _ => VIRTUAL_KEY(0),
    }
}

pub fn handle_key_event(down: bool, key: u32, get_last_key_input: impl Fn(u32) -> bool) -> C2S {
    let keysym = xkeysym::Keysym::from(key);
    let input = {
        let mut w_vk = map_xk_to_wvk(keysym);
        let key_char = keysym.key_char();
        if key_char.is_none() {
            warn!("key event: can't translate, key: 0x{:X}", key);
        }
        let c = key_char.unwrap_or(Default::default());

        info!("key event: modifier key: 0x{:X} -> 0x{:X}", key, w_vk.0);

        if keysym.is_modifier_key() {
            let b = get_last_key_input(key);
            if b == down {
                info!("key event: skip modifier key: 0x{:X}", key);
                return C2S::KeyEvent { down, key };
            }
        }

        if c.is_ascii() && w_vk.0 == 0 {
            let buf = &mut [0; 1];
            c.encode_utf8(buf);
            unsafe {
                let short = VkKeyScanA(buf[0] as i8);
                let lower = (short & 0xFF) as u16;
                w_vk = VIRTUAL_KEY(lower);
            }
            info!("key event: ascii: 0x{:X} -> 0x{:X}", c as u16, w_vk.0);
        }

        let mut scan = 0;
        let mut dw_flags = if down {
            KEYBD_EVENT_FLAGS(0)
        } else {
            KEYEVENTF_KEYUP
        };
        if w_vk.0 == 0 {
            dw_flags |= KEYEVENTF_UNICODE;
            info!("key event: unicode: 0x{:X}", c as u16);
            scan = c as u16;
        }
        info!("key char: {:?}", key_char);
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: w_vk,
                    wScan: scan,
                    dwFlags: dw_flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    };
    unsafe {
        info!("key event: {:?}", input.Anonymous.ki);
    }
    let input_array = [input];
    let send_input = unsafe { SendInput(&input_array, size_of::<INPUT>() as i32) };
    unsafe {
        if send_input == 0 {
            let last_error = GetLastError();
            error!("SendInput failed with error: {:?}", last_error);
        }
    }
    C2S::KeyEvent { down, key }
}

pub fn handle_clipboard_paste(text: String) -> anyhow::Result<()> {
    set_clipboard_string(text.as_str()).map_err(|e| anyhow::anyhow!(e))
}
