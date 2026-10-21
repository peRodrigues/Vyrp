//! Own every duplicated clipboard handle until ownership passes to Windows.
//! Unsupported private/owner-display data aborts BEFORE EmptyClipboard; never
//! discard a format just because it cannot be copied safely.
use crate::core::types::InsertRequest;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{GetLastError, GlobalFree, HANDLE, HWND, SetLastError};
use windows_sys::Win32::Graphics::Gdi::{
    CopyEnhMetaFileW, DeleteEnhMetaFile, DeleteMetaFile, DeleteObject,
};
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Ole::OleDuplicateData;

pub struct Open;
impl Open {
    pub fn acquire(owner: HWND, deadline: Instant) -> Result<Self, String> {
        loop {
            if Instant::now() >= deadline {
                return Err("clipboard ocupado (timeout)".into());
            }
            // SAFETY: owner is the clipboard worker's live message-only window.
            if unsafe { OpenClipboard(owner) } != 0 {
                return Ok(Self);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

pub struct Data {
    format: u32,
    handle: HANDLE,
}
impl Drop for Data {
    fn drop(&mut self) {
        if self.handle.is_null() {
            return;
        }
        // SAFETY: only duplicated/allocated handles owned by this struct are freed.
        unsafe {
            match self.format {
                2 | 9 | 0x82 => {
                    DeleteObject(self.handle);
                }
                14 | 0x8E => {
                    DeleteEnhMetaFile(self.handle);
                }
                3 | 0x83 => {
                    let p = GlobalLock(self.handle) as *const METAFILEPICT;
                    if !p.is_null() {
                        DeleteMetaFile((*p).hMF);
                        GlobalUnlock(self.handle);
                    }
                    GlobalFree(self.handle);
                }
                _ => {
                    GlobalFree(self.handle);
                }
            }
        }
    }
}

impl Data {
    fn bytes(format: u32, bytes: &[u8]) -> Result<Self, String> {
        // SAFETY: allocate/copy exact byte length; RAII frees on failure.
        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len().max(1));
            if handle.is_null() {
                return Err("memória do clipboard indisponível".into());
            }
            let data = Self { format, handle };
            let target = GlobalLock(handle).cast::<u8>();
            if target.is_null() {
                return Err("GlobalLock falhou".into());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
            GlobalUnlock(handle);
            Ok(data)
        }
    }
    fn publish(&mut self) -> Result<(), String> {
        // SAFETY: clipboard open; system owns handle only after success.
        if unsafe { SetClipboardData(self.format, self.handle) }.is_null() {
            return Err(format!(
                "SetClipboardData falhou no formato {}",
                self.format
            ));
        }
        self.handle = std::ptr::null_mut();
        Ok(())
    }
}

pub struct Snapshot {
    data: Vec<Data>,
}
impl Snapshot {
    pub fn capture(_open: &Open, deadline: Instant) -> Result<Self, String> {
        let mut data = Vec::new();
        let mut format = 0;
        let mut bytes = 0usize;
        // SAFETY: clipboard locked, duplicated handles are independent of its owner.
        unsafe {
            loop {
                if Instant::now() >= deadline {
                    return Err("snapshot excedeu o prazo".into());
                }
                SetLastError(0);
                format = EnumClipboardFormats(format);
                if format == 0 {
                    if GetLastError() != 0 {
                        return Err("EnumClipboardFormats falhou".into());
                    }
                    break;
                }
                if data.len() >= 256 || format == 0x80 || (0x200..=0x3ff).contains(&format) {
                    return Err(
                        "clipboard contém formato privado ou owner-display não duplicável".into(),
                    );
                }
                let source = GetClipboardData(format); // may trigger delayed rendering
                if source.is_null() {
                    return Err(format!("formato {format} indisponível"));
                }
                let handle = match format {
                    14 | 0x8e => CopyEnhMetaFileW(source, std::ptr::null()),
                    2 | 3 | 9 | 0x82 | 0x83 => {
                        let base = match format {
                            0x82 => 2,
                            0x83 => 3,
                            n => n,
                        };
                        OleDuplicateData(source, base as u16, 0)
                    }
                    _ => {
                        let size = GlobalSize(source);
                        bytes = bytes.saturating_add(size);
                        if size == 0 || bytes > 64 * 1024 * 1024 {
                            return Err("clipboard não duplicável ou maior que 64 MiB".into());
                        }
                        OleDuplicateData(source, format as u16, 0)
                    }
                };
                if handle.is_null() {
                    return Err(format!("cópia do formato {format} falhou"));
                }
                data.push(Data { format, handle });
            }
        }
        Ok(Self { data })
    }

    pub fn restore(mut self, _open: &Open) -> Result<(), String> {
        if unsafe { EmptyClipboard() } == 0 {
            return Err("EmptyClipboard falhou ao restaurar".into());
        }
        let mut error = None;
        for data in &mut self.data {
            if let Err(e) = data.publish() {
                error = Some(e);
            }
        }
        error.map_or(Ok(()), Err)
    }
}

fn registered(name: &str) -> Result<u32, String> {
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let id = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
    if id == 0 {
        Err("RegisterClipboardFormat falhou".into())
    } else {
        Ok(id)
    }
}

/// Prepare allocations before emptying clipboard. No external data is stored.
pub fn prepare(req: &InsertRequest) -> Result<Vec<Data>, String> {
    let bytes: Vec<u8> = req
        .text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut data = vec![Data::bytes(13, &bytes)?];
    if let Some(html) = &req.html {
        let mut bytes = super::html_fragment(html).into_bytes();
        bytes.push(0);
        data.push(Data::bytes(registered("HTML Format")?, &bytes)?);
    }
    for name in ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"] {
        data.push(Data::bytes(registered(name)?, &0u32.to_le_bytes())?);
    }
    data.push(Data::bytes(
        registered("ExcludeClipboardContentFromMonitorProcessing")?,
        &[0],
    )?);
    Ok(data)
}

pub fn publish(mut data: Vec<Data>, _open: &Open) -> Result<(), String> {
    if unsafe { EmptyClipboard() } == 0 {
        return Err("EmptyClipboard falhou".into());
    }
    for item in &mut data {
        item.publish()?;
    }
    Ok(())
}

pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

pub fn owned_by(owner: HWND) -> bool {
    unsafe { GetClipboardOwner() == owner }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Graphics::Gdi::{CloseEnhMetaFile, CreateBitmap, CreateEnhMetaFileW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE,
    };

    // Explicit opt-in only: touches the real clipboard, but never injects keys.
    // One lock covers the whole transaction, and the user's snapshot is restored
    // even if a fixture assertion panics. Run only with the test slot released.
    #[test]
    #[ignore = "clipboard real; exige slot de teste liberado"]
    fn native_formats_roundtrip_restores_user_clipboard() {
        unsafe {
            let class: Vec<u16> = "STATIC".encode_utf16().chain([0]).collect();
            let owner = CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!owner.is_null());
            let deadline = Instant::now() + Duration::from_secs(3);
            let open = Open::acquire(owner, deadline).unwrap();
            let user = Snapshot::capture(&open, deadline)
                .expect("snapshot inicial falhou; clipboard não alterado");
            let result = std::panic::catch_unwind(|| {
                let req = InsertRequest {
                    text: "fixture ação 🦀".into(),
                    html: Some("<b>ação 🦀</b>".into()),
                    ..Default::default()
                };
                let mut fixture = prepare(&req).unwrap();
                let rtf_id = registered("Rich Text Format").unwrap();
                fixture.push(Data::bytes(rtf_id, b"{\\rtf1 fixture}\0").unwrap());
                let bitmap = CreateBitmap(1, 1, 1, 32, [0u8, 0, 255, 0].as_ptr().cast());
                assert!(!bitmap.is_null());
                fixture.push(Data {
                    format: 2,
                    handle: bitmap,
                });
                let dc = CreateEnhMetaFileW(
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                );
                assert!(!dc.is_null());
                let metafile = CloseEnhMetaFile(dc);
                assert!(!metafile.is_null());
                fixture.push(Data {
                    format: 14,
                    handle: metafile,
                });
                publish(fixture, &open).unwrap();
                let original = Snapshot::capture(&open, deadline).unwrap();
                let count = original.data.len();
                assert!(count >= 8);
                let mut expected = std::collections::BTreeMap::new();
                for item in &original.data {
                    if ![2, 3, 9, 14, 0x82, 0x83, 0x8e].contains(&item.format) {
                        let len = GlobalSize(item.handle);
                        let ptr = GlobalLock(item.handle).cast::<u8>();
                        assert!(!ptr.is_null());
                        expected.insert(item.format, std::slice::from_raw_parts(ptr, len).to_vec());
                        GlobalUnlock(item.handle);
                    }
                }
                publish(
                    prepare(&InsertRequest {
                        text: "temporary dictation".into(),
                        ..Default::default()
                    })
                    .unwrap(),
                    &open,
                )
                .unwrap();
                original.restore(&open).unwrap();
                let restored = Snapshot::capture(&open, deadline).unwrap();
                assert_eq!(restored.data.len(), count);
                for item in &restored.data {
                    if let Some(bytes) = expected.get(&item.format) {
                        let ptr = GlobalLock(item.handle).cast::<u8>();
                        assert!(!ptr.is_null());
                        let equal =
                            std::slice::from_raw_parts(ptr, GlobalSize(item.handle)) == bytes;
                        GlobalUnlock(item.handle);
                        assert!(equal, "formato {} mudou", item.format);
                    }
                }
            });
            let restored = user.restore(&open);
            drop(open);
            DestroyWindow(owner);
            restored.expect("não foi possível restaurar clipboard original");
            if let Err(panic) = result {
                std::panic::resume_unwind(panic);
            }
        }
    }
}
