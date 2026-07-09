#[cfg(not(target_os = "windows"))]
pub fn probe_hardware_h264_encoder_count() -> anyhow::Result<u32> {
    anyhow::bail!("Media Foundation H.264 encoding is only available on Windows")
}

#[cfg(not(target_os = "windows"))]
pub fn probe_hardware_h264_decoder_count() -> anyhow::Result<u32> {
    anyhow::bail!("Media Foundation H.264 decoding is only available on Windows")
}

#[cfg(target_os = "windows")]
pub fn probe_hardware_h264_encoder_count() -> anyhow::Result<u32> {
    windows::probe_h264_transform_count(
        windows::MFT_CATEGORY_VIDEO_ENCODER,
        windows::MF_MEDIA_TYPE_VIDEO,
        windows::GUID::default(),
        windows::MF_MEDIA_TYPE_VIDEO,
        windows::MF_VIDEO_FORMAT_H264,
    )
}

#[cfg(target_os = "windows")]
pub fn probe_hardware_h264_decoder_count() -> anyhow::Result<u32> {
    windows::probe_h264_transform_count(
        windows::MFT_CATEGORY_VIDEO_DECODER,
        windows::MF_MEDIA_TYPE_VIDEO,
        windows::MF_VIDEO_FORMAT_H264,
        windows::MF_MEDIA_TYPE_VIDEO,
        windows::GUID::default(),
    )
}

#[cfg(target_os = "windows")]
mod windows {
    use std::{ffi::c_void, mem, ptr};

    use anyhow::Context;
    pub use windows_sys::core::GUID;
    use windows_sys::{
        Win32::{
            Foundation::HMODULE,
            System::LibraryLoader::{GetProcAddress, LoadLibraryA},
        },
        core::{HRESULT, IUnknown_Vtbl},
    };

    const MF_VERSION: u32 = 0x0002_0070;
    const MFSTARTUP_FULL: u32 = 0;
    const MFT_ENUM_FLAG_HARDWARE: u32 = 0x0000_0004;
    const MFT_ENUM_FLAG_SORTANDFILTER: u32 = 0x0000_0040;
    pub const MFT_CATEGORY_VIDEO_DECODER: GUID =
        GUID::from_u128(0xd6c02d4b_6833_45b4_971a_05a4b04bab91);
    pub const MFT_CATEGORY_VIDEO_ENCODER: GUID =
        GUID::from_u128(0xf79eac7d_e545_4387_bdee_d647d7bde42a);
    pub const MF_MEDIA_TYPE_VIDEO: GUID = GUID::from_u128(0x73646976_0000_0010_8000_00aa00389b71);
    pub const MF_VIDEO_FORMAT_H264: GUID = GUID::from_u128(0x34363248_0000_0010_8000_00aa00389b71);

    type MFStartupFn = unsafe extern "system" fn(u32, u32) -> HRESULT;
    type MFShutdownFn = unsafe extern "system" fn() -> HRESULT;
    type MFTEnumExFn = unsafe extern "system" fn(
        GUID,
        u32,
        *const MftRegisterTypeInfo,
        *const MftRegisterTypeInfo,
        *mut *mut *mut c_void,
        *mut u32,
    ) -> HRESULT;
    type CoTaskMemFreeFn = unsafe extern "system" fn(*const c_void);

    #[repr(C)]
    struct MftRegisterTypeInfo {
        guid_major_type: GUID,
        guid_sub_type: GUID,
    }

    pub fn probe_h264_transform_count(
        category: GUID,
        input_major_type: GUID,
        input_sub_type: GUID,
        output_major_type: GUID,
        output_sub_type: GUID,
    ) -> anyhow::Result<u32> {
        let mfplat = load_library(b"mfplat.dll\0").context("failed to load mfplat.dll")?;
        let startup: MFStartupFn = load_proc(mfplat, b"MFStartup\0")?;
        let shutdown: MFShutdownFn = load_proc(mfplat, b"MFShutdown\0")?;
        let enum_ex: MFTEnumExFn = load_proc(mfplat, b"MFTEnumEx\0")?;

        hr_result("MFStartup", unsafe { startup(MF_VERSION, MFSTARTUP_FULL) })?;
        let _guard = MediaFoundationShutdown { shutdown };

        let input_type = MftRegisterTypeInfo {
            guid_major_type: input_major_type,
            guid_sub_type: input_sub_type,
        };
        let output_type = MftRegisterTypeInfo {
            guid_major_type: output_major_type,
            guid_sub_type: output_sub_type,
        };
        let mut activates: *mut *mut c_void = ptr::null_mut();
        let mut count = 0_u32;
        hr_result("MFTEnumEx", unsafe {
            enum_ex(
                category,
                MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
                &input_type,
                &output_type,
                &mut activates,
                &mut count,
            )
        })?;

        unsafe {
            release_activates(activates, count);
            free_cotaskmem(activates.cast());
        }
        Ok(count)
    }

    struct MediaFoundationShutdown {
        shutdown: MFShutdownFn,
    }

    impl Drop for MediaFoundationShutdown {
        fn drop(&mut self) {
            unsafe {
                let _ = (self.shutdown)();
            }
        }
    }

    fn load_library(name: &'static [u8]) -> anyhow::Result<HMODULE> {
        let module = unsafe { LoadLibraryA(name.as_ptr()) };
        if module.is_null() {
            anyhow::bail!(
                "LoadLibraryA({}) failed",
                String::from_utf8_lossy(c_string_name(name))
            );
        }
        Ok(module)
    }

    fn load_proc<T: Copy>(module: HMODULE, name: &'static [u8]) -> anyhow::Result<T> {
        let proc = unsafe { GetProcAddress(module, name.as_ptr()) };
        let Some(proc) = proc else {
            anyhow::bail!(
                "GetProcAddress({}) failed",
                String::from_utf8_lossy(c_string_name(name))
            );
        };
        Ok(unsafe { mem::transmute_copy(&proc) })
    }

    fn c_string_name(bytes: &'static [u8]) -> &'static [u8] {
        bytes.strip_suffix(&[0]).unwrap_or(bytes)
    }

    fn hr_result(action: &str, hr: HRESULT) -> anyhow::Result<()> {
        if hr >= 0 {
            Ok(())
        } else {
            anyhow::bail!("{action} failed with HRESULT 0x{:08x}", hr as u32)
        }
    }

    unsafe fn release_activates(activates: *mut *mut c_void, count: u32) {
        if activates.is_null() {
            return;
        }
        for index in 0..count as usize {
            let activate = unsafe { *activates.add(index) };
            if activate.is_null() {
                continue;
            }
            let vtbl = unsafe { *(activate as *mut *mut IUnknown_Vtbl) };
            if !vtbl.is_null() {
                unsafe {
                    ((*vtbl).Release)(activate);
                }
            }
        }
    }

    unsafe fn free_cotaskmem(memory: *const c_void) {
        if memory.is_null() {
            return;
        }
        let Ok(ole32) = load_library(b"ole32.dll\0") else {
            return;
        };
        let Ok(free) = load_proc::<CoTaskMemFreeFn>(ole32, b"CoTaskMemFree\0") else {
            return;
        };
        unsafe {
            free(memory);
        }
    }
}
