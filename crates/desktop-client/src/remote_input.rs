use teamview_protocol::control::{RemoteInputEvent, RemoteInputKind};

pub trait RemoteInputApplier {
    fn output_mode(&self) -> &'static str;
    fn applied_events(&self) -> u64;
    fn apply(&mut self, event: &RemoteInputEvent) -> anyhow::Result<()>;
}

#[derive(Debug, Default)]
pub struct LoggingRemoteInputApplier {
    applied_events: u64,
}

impl RemoteInputApplier for LoggingRemoteInputApplier {
    fn output_mode(&self) -> &'static str {
        "log"
    }

    fn applied_events(&self) -> u64 {
        self.applied_events
    }

    fn apply(&mut self, _event: &RemoteInputEvent) -> anyhow::Result<()> {
        self.applied_events = self.applied_events.saturating_add(1);
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct NativeRemoteInputApplier {
    applied_events: u64,
}

impl NativeRemoteInputApplier {
    pub fn new() -> anyhow::Result<Self> {
        ensure_native_input_supported()?;
        Ok(Self::default())
    }
}

impl RemoteInputApplier for NativeRemoteInputApplier {
    fn output_mode(&self) -> &'static str {
        "native"
    }

    fn applied_events(&self) -> u64 {
        self.applied_events
    }

    fn apply(&mut self, event: &RemoteInputEvent) -> anyhow::Result<()> {
        send_native_input(&event.kind)?;
        self.applied_events = self.applied_events.saturating_add(1);
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn ensure_native_input_supported() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn ensure_native_input_supported() -> anyhow::Result<()> {
    anyhow::bail!("native remote input output is only available on Windows")
}

#[cfg(target_os = "windows")]
fn send_native_input(kind: &RemoteInputKind) -> anyhow::Result<()> {
    windows_input::send(kind)
}

#[cfg(not(target_os = "windows"))]
fn send_native_input(_kind: &RemoteInputKind) -> anyhow::Result<()> {
    anyhow::bail!("native remote input output is only available on Windows")
}

#[cfg(target_os = "windows")]
mod windows_input {
    use std::mem;

    use teamview_protocol::control::{PointerButton, RemoteInputKind};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
        MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
        MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput,
    };

    const XBUTTON1: u32 = 0x0001;
    const XBUTTON2: u32 = 0x0002;

    pub fn send(kind: &RemoteInputKind) -> anyhow::Result<()> {
        let inputs = inputs_for_kind(kind)?;
        send_inputs(&inputs)
    }

    pub(super) fn inputs_for_kind(kind: &RemoteInputKind) -> anyhow::Result<Vec<INPUT>> {
        match kind {
            RemoteInputKind::PointerMove {
                normalized_x,
                normalized_y,
            } => Ok(vec![absolute_move_input(*normalized_x, *normalized_y)]),
            RemoteInputKind::PointerButton {
                button,
                pressed,
                normalized_x,
                normalized_y,
            } => Ok(vec![
                absolute_move_input(*normalized_x, *normalized_y),
                pointer_button_input(*button, *pressed),
            ]),
            RemoteInputKind::PointerWheel {
                delta_x,
                delta_y,
                normalized_x,
                normalized_y,
            } => {
                let mut inputs = vec![absolute_move_input(*normalized_x, *normalized_y)];
                if *delta_y != 0 {
                    inputs.push(mouse_input(0, 0, *delta_y as i32 as u32, MOUSEEVENTF_WHEEL));
                }
                if *delta_x != 0 {
                    inputs.push(mouse_input(
                        0,
                        0,
                        *delta_x as i32 as u32,
                        MOUSEEVENTF_HWHEEL,
                    ));
                }
                Ok(inputs)
            }
            RemoteInputKind::Key { key_code, pressed } => {
                if *key_code == 0 {
                    anyhow::bail!("remote input key_code must be non-zero");
                }
                Ok(vec![keyboard_input(*key_code, *pressed)])
            }
            RemoteInputKind::Text { text } => {
                let mut inputs = Vec::new();
                for code_unit in text.encode_utf16() {
                    inputs.push(unicode_input(code_unit, true));
                    inputs.push(unicode_input(code_unit, false));
                }
                Ok(inputs)
            }
        }
    }

    pub(super) fn absolute_axis(value: u16) -> i32 {
        value as i32
    }

    pub(super) fn pointer_button_flags(button: PointerButton, pressed: bool) -> (u32, u32) {
        match (button, pressed) {
            (PointerButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (PointerButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (PointerButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (PointerButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (PointerButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (PointerButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (PointerButton::X1, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
            (PointerButton::X1, false) => (MOUSEEVENTF_XUP, XBUTTON1),
            (PointerButton::X2, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
            (PointerButton::X2, false) => (MOUSEEVENTF_XUP, XBUTTON2),
        }
    }

    fn absolute_move_input(normalized_x: u16, normalized_y: u16) -> INPUT {
        mouse_input(
            absolute_axis(normalized_x),
            absolute_axis(normalized_y),
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        )
    }

    fn pointer_button_input(button: PointerButton, pressed: bool) -> INPUT {
        let (flags, mouse_data) = pointer_button_flags(button, pressed);
        mouse_input(0, 0, mouse_data, flags)
    }

    fn mouse_input(dx: i32, dy: i32, mouse_data: u32, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: mouse_data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn keyboard_input(key_code: u16, pressed: bool) -> INPUT {
        let flags = if pressed { 0 } else { KEYEVENTF_KEYUP };
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key_code,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn unicode_input(code_unit: u16, pressed: bool) -> INPUT {
        let flags = if pressed {
            KEYEVENTF_UNICODE
        } else {
            KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
        };
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: code_unit,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn send_inputs(inputs: &[INPUT]) -> anyhow::Result<()> {
        if inputs.is_empty() {
            return Ok(());
        }
        let sent = unsafe {
            SendInput(
                inputs.len().min(u32::MAX as usize) as u32,
                inputs.as_ptr(),
                mem::size_of::<INPUT>() as i32,
            )
        };
        if sent != inputs.len() as u32 {
            anyhow::bail!("SendInput injected {sent}/{} input events", inputs.len());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use teamview_protocol::control::{PointerButton, RemoteInputEvent, RemoteInputKind};

    use super::*;

    #[test]
    fn logging_remote_input_applier_counts_events() {
        let event = RemoteInputEvent {
            sender_user_id: 7,
            sequence_number: 1,
            event_time_micros: 2,
            kind: RemoteInputKind::Key {
                key_code: 13,
                pressed: true,
            },
        };
        let mut applier = LoggingRemoteInputApplier::default();

        applier.apply(&event).unwrap();
        applier.apply(&event).unwrap();

        assert_eq!(applier.output_mode(), "log");
        assert_eq!(applier.applied_events(), 2);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_absolute_axis_uses_protocol_coordinate_space() {
        assert_eq!(windows_input::absolute_axis(0), 0);
        assert_eq!(windows_input::absolute_axis(32_768), 32_768);
        assert_eq!(windows_input::absolute_axis(u16::MAX), 65_535);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_pointer_button_flags_map_x_buttons() {
        let (down_flags, down_data) = windows_input::pointer_button_flags(PointerButton::X1, true);
        let (up_flags, up_data) = windows_input::pointer_button_flags(PointerButton::X2, false);

        assert_ne!(down_flags, up_flags);
        assert_eq!(down_data, 1);
        assert_eq!(up_data, 2);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_text_input_expands_to_key_down_and_up() {
        let inputs = windows_input::inputs_for_kind(&RemoteInputKind::Text {
            text: "hi".to_owned(),
        })
        .unwrap();

        assert_eq!(inputs.len(), 4);
    }
}
