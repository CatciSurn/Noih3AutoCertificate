//! A native, modal preparation checklist with the reference image in the same window.

use std::path::Path;

pub fn show(image_path: &Path) -> crate::Result<bool> {
    #[cfg(windows)]
    {
        windows::show(image_path)
    }
    #[cfg(not(windows))]
    {
        let _ = image_path;
        Err("准备确认弹窗仅支持 Windows。".into())
    }
}

#[cfg(windows)]
mod windows {
    use crate::Result;
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    type Handle = *mut c_void;
    const ID_OK: usize = 1;
    const ID_CANCEL: usize = 2;
    const ID_CHECKLIST: usize = 100;
    const ID_CAPTION: usize = 101;
    const USER_DATA: i32 = -21;
    const WINDOW_STYLE: u32 = 0x80000000 | 0x00c00000 | 0x00080000 | 0x02000000 | 0x00000080;
    const CHECKLIST: &str = "请先确认：\r\n1. 已在目标账号下新建游戏存档。\r\n2. 游戏主界面的 CONTINUE 和 LOAD GAME 可点击。\r\n3. 已退出游戏，避免游戏写入存档。";

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct MonitorInfo {
        size: u32,
        monitor: Rect,
        work: Rect,
        flags: u32,
    }

    #[repr(C)]
    struct PaintStruct {
        dc: Handle,
        erase: i32,
        paint: Rect,
        restore: i32,
        incremental_update: i32,
        reserved: [u8; 32],
    }

    #[repr(C)]
    struct StartupInput {
        version: u32,
        debug_callback: *const c_void,
        suppress_background_thread: i32,
        suppress_external_codecs: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn DialogBoxIndirectParamW(
            instance: Handle,
            template: *const c_void,
            parent: Handle,
            callback: unsafe extern "system" fn(Handle, u32, usize, isize) -> isize,
            parameter: isize,
        ) -> isize;
        fn EndDialog(window: Handle, result: isize) -> i32;
        fn SetWindowLongPtrW(window: Handle, index: i32, value: isize) -> isize;
        fn GetWindowLongPtrW(window: Handle, index: i32) -> isize;
        fn CreateWindowExW(
            ex_style: u32,
            class: *const u16,
            title: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: Handle,
            menu: Handle,
            instance: Handle,
            parameter: *mut c_void,
        ) -> Handle;
        fn GetDlgItem(window: Handle, id: i32) -> Handle;
        fn SendMessageW(window: Handle, message: u32, wparam: usize, lparam: isize) -> isize;
        fn SetWindowPos(
            window: Handle,
            after: Handle,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
        fn SetFocus(window: Handle) -> Handle;
        fn AdjustWindowRectExForDpi(
            rect: *mut Rect,
            style: u32,
            menu: i32,
            ex: u32,
            dpi: u32,
        ) -> i32;
        fn MonitorFromWindow(window: Handle, flags: u32) -> Handle;
        fn GetMonitorInfoW(monitor: Handle, info: *mut MonitorInfo) -> i32;
        fn GetDpiForWindow(window: Handle) -> u32;
        fn SetThreadDpiAwarenessContext(context: Handle) -> Handle;
        fn GetDC(window: Handle) -> Handle;
        fn ReleaseDC(window: Handle, dc: Handle) -> i32;
        fn DrawTextW(dc: Handle, text: *const u16, count: i32, rect: *mut Rect, format: u32)
            -> i32;
        fn BeginPaint(window: Handle, paint: *mut PaintStruct) -> Handle;
        fn EndPaint(window: Handle, paint: *const PaintStruct) -> i32;
        fn FillRect(dc: Handle, rect: *const Rect, brush: Handle) -> i32;
        fn GetClientRect(window: Handle, rect: *mut Rect) -> i32;
        fn InvalidateRect(window: Handle, rect: *const Rect, erase: i32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> Handle;
        fn GetConsoleWindow() -> Handle;
        fn LoadLibraryExW(name: *const u16, file: Handle, flags: u32) -> Handle;
        fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
        fn FreeLibrary(module: Handle) -> i32;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateFontW(
            height: i32,
            width: i32,
            escapement: i32,
            orientation: i32,
            weight: i32,
            italic: u32,
            underline: u32,
            strikeout: u32,
            charset: u32,
            output_precision: u32,
            clip_precision: u32,
            quality: u32,
            pitch_family: u32,
            face: *const u16,
        ) -> Handle;
        fn DeleteObject(object: Handle) -> i32;
        fn SelectObject(dc: Handle, object: Handle) -> Handle;
        fn GetStockObject(object: i32) -> Handle;
        fn SetBkMode(dc: Handle, mode: i32) -> i32;
        fn SetTextColor(dc: Handle, color: u32) -> u32;
    }

    struct LoadedModule(Handle);
    impl Drop for LoadedModule {
        fn drop(&mut self) {
            unsafe {
                FreeLibrary(self.0);
            }
        }
    }

    // Resolve the system DLL directly: no external PNG library or GNU import library needed.
    struct GdiApi {
        _module: LoadedModule,
        startup: unsafe extern "system" fn(*mut usize, *const StartupInput, *mut c_void) -> i32,
        shutdown: unsafe extern "system" fn(usize),
        load_image: unsafe extern "system" fn(*const u16, *mut Handle) -> i32,
        image_width: unsafe extern "system" fn(Handle, *mut u32) -> i32,
        image_height: unsafe extern "system" fn(Handle, *mut u32) -> i32,
        dispose_image: unsafe extern "system" fn(Handle) -> i32,
        create_graphics: unsafe extern "system" fn(Handle, *mut Handle) -> i32,
        delete_graphics: unsafe extern "system" fn(Handle) -> i32,
        interpolation: unsafe extern "system" fn(Handle, i32) -> i32,
        draw_image: unsafe extern "system" fn(Handle, Handle, i32, i32, i32, i32) -> i32,
    }

    impl GdiApi {
        unsafe fn load() -> Result<Self> {
            let module = LoadLibraryExW(wide("gdiplus.dll").as_ptr(), null_mut(), 0x00000800);
            if module.is_null() {
                return Err(win_error("无法加载系统参考图片显示组件"));
            }
            let module = LoadedModule(module);
            let symbol = |name: &[u8]| -> Result<*mut c_void> {
                let address = GetProcAddress(module.0, name.as_ptr());
                if address.is_null() {
                    Err(win_error("系统参考图片显示组件缺少所需函数"))
                } else {
                    Ok(address)
                }
            };
            Ok(Self {
                startup: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "system" fn(*mut usize, *const StartupInput, *mut c_void) -> i32,
                >(symbol(b"GdiplusStartup\0")?),
                shutdown: std::mem::transmute::<*mut c_void, unsafe extern "system" fn(usize)>(
                    symbol(b"GdiplusShutdown\0")?,
                ),
                load_image: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "system" fn(*const u16, *mut Handle) -> i32,
                >(symbol(b"GdipLoadImageFromFile\0")?),
                image_width: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "system" fn(Handle, *mut u32) -> i32,
                >(symbol(b"GdipGetImageWidth\0")?),
                image_height: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "system" fn(Handle, *mut u32) -> i32,
                >(symbol(b"GdipGetImageHeight\0")?),
                dispose_image: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "system" fn(Handle) -> i32,
                >(symbol(b"GdipDisposeImage\0")?),
                create_graphics: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "system" fn(Handle, *mut Handle) -> i32,
                >(symbol(b"GdipCreateFromHDC\0")?),
                delete_graphics: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "system" fn(Handle) -> i32,
                >(symbol(b"GdipDeleteGraphics\0")?),
                interpolation: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "system" fn(Handle, i32) -> i32,
                >(symbol(b"GdipSetInterpolationMode\0")?),
                draw_image: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "system" fn(Handle, Handle, i32, i32, i32, i32) -> i32,
                >(symbol(b"GdipDrawImageRectI\0")?),
                _module: module,
            })
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn win_error(action: &str) -> String {
        format!("{action}：{}", std::io::Error::last_os_error())
    }

    struct GdiSession<'a>(&'a GdiApi, usize);
    impl Drop for GdiSession<'_> {
        fn drop(&mut self) {
            unsafe { (self.0.shutdown)(self.1) }
        }
    }

    struct Image<'a>(Handle, &'a GdiApi);
    impl Drop for Image<'_> {
        fn drop(&mut self) {
            unsafe {
                (self.1.dispose_image)(self.0);
            }
        }
    }

    struct Graphics<'a>(Handle, &'a GdiApi);
    impl Drop for Graphics<'_> {
        fn drop(&mut self) {
            unsafe {
                (self.1.delete_graphics)(self.0);
            }
        }
    }

    struct DpiContext(Handle);
    impl Drop for DpiContext {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    SetThreadDpiAwarenessContext(self.0);
                }
            }
        }
    }

    struct State<'a> {
        image: Image<'a>,
        image_width: u32,
        image_height: u32,
        image_path: String,
        image_rect: Cell<Rect>,
        font: Cell<Handle>,
        ready: Cell<bool>,
        laying_out: Cell<bool>,
        error: RefCell<Option<String>>,
    }

    impl Drop for State<'_> {
        fn drop(&mut self) {
            // The modal call has destroyed all child windows before their font and image.
            if !self.font.get().is_null() {
                unsafe {
                    DeleteObject(self.font.get());
                }
            }
        }
    }

    pub(super) fn show(image_path: &Path) -> Result<bool> {
        if !image_path.is_file() {
            return Err(format!(
                "找不到参考图片：{}。请将 image.png 放在程序旁边。",
                image_path.display()
            ));
        }
        let path: Vec<u16> = image_path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if path[..path.len() - 1].contains(&0) {
            return Err(format!(
                "参考图片路径包含无效字符：{}",
                image_path.display()
            ));
        }
        unsafe {
            // Restore the caller's DPI context once the modal window has been destroyed.
            let _dpi = DpiContext(SetThreadDpiAwarenessContext(-4isize as Handle));
            let api = GdiApi::load()?;
            let input = StartupInput {
                version: 1,
                debug_callback: null(),
                suppress_background_thread: 0,
                suppress_external_codecs: 0,
            };
            let mut token = 0;
            let status = (api.startup)(&mut token, &input, null_mut());
            if status != 0 {
                return Err(format!(
                    "无法初始化参考图片显示组件（GDI+ 错误 {status}）。"
                ));
            }
            let _session = GdiSession(&api, token);
            let mut image = null_mut();
            let status = (api.load_image)(path.as_ptr(), &mut image);
            if status != 0 || image.is_null() {
                if !image.is_null() {
                    (api.dispose_image)(image);
                }
                return Err(format!(
                    "无法加载参考图片：{}（GDI+ 错误 {status}）。",
                    image_path.display()
                ));
            }
            let image = Image(image, &api);
            let (mut width, mut height) = (0, 0);
            if (api.image_width)(image.0, &mut width) != 0
                || (api.image_height)(image.0, &mut height) != 0
                || width == 0
                || height == 0
            {
                return Err(format!("参考图片尺寸无效：{}", image_path.display()));
            }
            let state = State {
                image,
                image_width: width,
                image_height: height,
                image_path: image_path.display().to_string(),
                image_rect: Cell::new(Rect::default()),
                font: Cell::new(null_mut()),
                ready: Cell::new(false),
                laying_out: Cell::new(false),
                error: RefCell::new(None),
            };
            let template = dialog_template();
            let result = DialogBoxIndirectParamW(
                GetModuleHandleW(null()),
                template.as_ptr().cast(),
                null_mut(),
                dialog_proc,
                &state as *const State as isize,
            );
            let system_error = (result == -1).then(|| win_error("无法打开开始前确认弹窗"));
            if let Some(error) = state.error.borrow_mut().take().or(system_error) {
                return Err(error);
            }
            Ok(result == ID_OK as isize)
        }
    }

    // A DLGTEMPLATE is packed to two-byte fields but must start on a DWORD boundary.
    // Serialize the documented fields, then store in u32s instead of relying on Rust padding.
    fn dialog_template() -> Vec<u32> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&WINDOW_STYLE.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        for value in [0u16, 0, 0, 400, 300, 0, 0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in wide("开始前确认") {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        bytes
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect()
    }

    unsafe fn state_for<'a>(window: Handle) -> Option<&'a State<'a>> {
        (GetWindowLongPtrW(window, USER_DATA) as *const State).as_ref()
    }

    unsafe extern "system" fn dialog_proc(
        window: Handle,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        // Never let a Rust panic unwind through a Windows callback.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle_message(window, message, wparam, lparam)
        })) {
            Ok(result) => result,
            Err(_) => {
                if let Some(state) = state_for(window) {
                    if let Ok(mut error) = state.error.try_borrow_mut() {
                        *error = Some("准备确认弹窗发生内部错误，操作已取消。".into());
                    }
                }
                EndDialog(window, ID_CANCEL as isize);
                1
            }
        }
    }

    unsafe fn handle_message(window: Handle, message: u32, wparam: usize, lparam: isize) -> isize {
        match message {
            0x0110 => {
                // WM_INITDIALOG
                SetWindowLongPtrW(window, USER_DATA, lparam);
                let state = state_for(window).ok_or("弹窗状态初始化失败。".to_string());
                let result = state.and_then(|state| initialize(window, state));
                if let Err(error) = result {
                    fail(window, error);
                }
                0 // initialize explicitly sets focus to Cancel.
            }
            0x0111 => {
                // WM_COMMAND; ID_CANCEL also receives Escape from the dialog manager.
                let command = wparam & 0xffff;
                if command == ID_CANCEL || command == ID_OK {
                    if command == ID_CANCEL || state_for(window).is_some_and(|s| s.ready.get()) {
                        EndDialog(window, command as isize);
                    }
                    return 1;
                }
                0
            }
            0x0010 => {
                // WM_CLOSE
                EndDialog(window, ID_CANCEL as isize);
                1
            }
            0x000f => {
                // WM_PAINT
                if let Some(state) = state_for(window) {
                    if let Err(error) = paint(window, state) {
                        fail(window, error);
                    }
                    return 1;
                }
                0
            }
            0x0138 | 0x0136 => {
                // WM_CTLCOLORSTATIC / WM_CTLCOLORDLG
                SetBkMode(wparam as Handle, 1);
                SetTextColor(wparam as Handle, 0x00232323);
                GetStockObject(0) as isize // WHITE_BRUSH is a borrowed stock object.
            }
            0x02e0 => {
                // WM_DPICHANGED
                if let Some(state) = state_for(window) {
                    if state.ready.get() && !state.laying_out.get() {
                        // First move onto the monitor selected by Windows, then fit its work area.
                        let suggested = &*(lparam as *const Rect);
                        SetWindowPos(
                            window,
                            null_mut(),
                            suggested.left,
                            suggested.top,
                            suggested.right - suggested.left,
                            suggested.bottom - suggested.top,
                            0x0014,
                        );
                        if let Err(error) = layout(window, state, (wparam & 0xffff) as u32) {
                            fail(window, error);
                        }
                    }
                }
                1
            }
            _ => 0,
        }
    }

    unsafe fn fail(window: Handle, error: String) {
        if let Some(state) = state_for(window) {
            *state.error.borrow_mut() = Some(error);
            state.ready.set(false);
        }
        EndDialog(window, ID_CANCEL as isize);
    }

    unsafe fn monitor_work(window: Handle) -> Result<Rect> {
        let mut info = MonitorInfo {
            size: size_of::<MonitorInfo>() as u32,
            monitor: Rect::default(),
            work: Rect::default(),
            flags: 0,
        };
        if GetMonitorInfoW(MonitorFromWindow(window, 2), &mut info) == 0 {
            return Err(win_error("无法获取屏幕可用区域"));
        }
        Ok(info.work)
    }

    unsafe fn initialize(window: Handle, state: &State) -> Result<()> {
        let work = monitor_work(GetConsoleWindow())?;
        SetWindowPos(
            window,
            null_mut(),
            work.left + 20,
            work.top + 20,
            0,
            0,
            0x0015,
        );
        for (id, class, text, style) in [
            (ID_CHECKLIST, "STATIC", CHECKLIST, 0x00000080), // SS_NOPREFIX, word wrapping.
            (
                ID_CAPTION,
                "STATIC",
                "参考图片（游戏主界面示意）",
                0x00000080,
            ),
            (ID_OK, "BUTTON", "我已准备好，继续", 0x00010000),
            (ID_CANCEL, "BUTTON", "取消", 0x00010001), // Default action is cancellation.
        ] {
            let child = CreateWindowExW(
                0,
                wide(class).as_ptr(),
                wide(text).as_ptr(),
                0x40000000 | 0x10000000 | style,
                0,
                0,
                1,
                1,
                window,
                id as Handle,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if child.is_null() {
                return Err(win_error("无法创建准备确认控件"));
            }
        }
        SendMessageW(window, 0x0401, ID_CANCEL, 0); // DM_SETDEFID
        layout(window, state, GetDpiForWindow(window).max(96))?;
        state.ready.set(true);
        SetFocus(GetDlgItem(window, ID_CANCEL as i32));
        Ok(())
    }

    unsafe fn layout(window: Handle, state: &State, dpi: u32) -> Result<()> {
        if state.laying_out.replace(true) {
            return Ok(());
        }
        struct LayoutGuard<'a>(&'a Cell<bool>);
        impl Drop for LayoutGuard<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _guard = LayoutGuard(&state.laying_out);
        let work = monitor_work(window)?;
        let work_width = work.right - work.left;
        let work_height = work.bottom - work.top;
        let mut frame = Rect::default();
        let style = GetWindowLongPtrW(window, -16) as u32;
        let ex_style = GetWindowLongPtrW(window, -20) as u32;
        if AdjustWindowRectExForDpi(&mut frame, style, 0, ex_style, dpi) == 0 {
            return Err(win_error("无法计算确认弹窗尺寸"));
        }
        let frame_width = frame.right - frame.left;
        let frame_height = frame.bottom - frame.top;
        // Keep controls readable at normal DPI, but also fit small displays or large scaling.
        let scale = (dpi as f64 / 96.0)
            .min((work_width - frame_width - 32) as f64 / 560.0)
            .min((work_height - frame_height - 32) as f64 / 430.0);
        if scale < 0.6 {
            return Err("屏幕可用区域过小，无法完整显示确认提示和参考图片。".into());
        }
        let px = |v: f64| (v * scale).round() as i32;
        let margin = px(24.0);
        let gap = px(12.0);
        let client_width = px(900.0).min(work_width - frame_width - 32);
        let content_width = client_width - 2 * margin;
        let new_font = CreateFontW(
            -px(16.0),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            0,
            wide("Microsoft YaHei UI").as_ptr(),
        );
        if new_font.is_null() {
            return Err(win_error("无法创建中文显示字体"));
        }
        // Child controls have switched fonts before the previous font is released.
        for id in [ID_CHECKLIST, ID_CAPTION, ID_OK, ID_CANCEL] {
            SendMessageW(GetDlgItem(window, id as i32), 0x0030, new_font as usize, 0);
        }
        let previous = state.font.replace(new_font);
        if !previous.is_null() {
            DeleteObject(previous);
        }
        let checklist_height = text_height(window, new_font, CHECKLIST, content_width)? + px(4.0);
        let caption_height = text_height(
            window,
            new_font,
            "参考图片（游戏主界面示意）",
            content_width,
        )?;
        let button_height = px(42.0);
        let fixed_height = 2 * margin + checklist_height + caption_height + button_height + 3 * gap;
        let available_image_height = work_height - frame_height - 32 - fixed_height;
        if available_image_height < px(80.0) {
            return Err("屏幕可用区域过小，无法完整显示参考图片。".into());
        }
        let image_scale = (content_width as f64 / state.image_width as f64)
            .min(available_image_height as f64 / state.image_height as f64);
        let image_width = (state.image_width as f64 * image_scale).floor() as i32;
        let image_height = (state.image_height as f64 * image_scale).floor() as i32;
        let image_top = margin + checklist_height + gap + caption_height + gap;
        let client_height = fixed_height + image_height;
        let width = client_width + frame_width;
        let height = client_height + frame_height;
        if SetWindowPos(
            window,
            null_mut(),
            work.left + (work_width - width) / 2,
            work.top + (work_height - height) / 2,
            width,
            height,
            0x0014,
        ) == 0
        {
            return Err(win_error("无法调整确认弹窗尺寸"));
        }
        place(
            window,
            ID_CHECKLIST,
            margin,
            margin,
            content_width,
            checklist_height,
        )?;
        place(
            window,
            ID_CAPTION,
            margin,
            margin + checklist_height + gap,
            content_width,
            caption_height,
        )?;
        state.image_rect.set(Rect {
            left: (client_width - image_width) / 2,
            top: image_top,
            right: (client_width - image_width) / 2 + image_width,
            bottom: image_top + image_height,
        });
        let cancel_width = px(100.0);
        let continue_width = px(210.0);
        let button_top = image_top + image_height + gap;
        place(
            window,
            ID_OK,
            client_width - margin - cancel_width - gap - continue_width,
            button_top,
            continue_width,
            button_height,
        )?;
        place(
            window,
            ID_CANCEL,
            client_width - margin - cancel_width,
            button_top,
            cancel_width,
            button_height,
        )?;
        InvalidateRect(window, null(), 1);
        Ok(())
    }

    unsafe fn text_height(window: Handle, font: Handle, text: &str, width: i32) -> Result<i32> {
        let text = wide(text);
        let dc = GetDC(window);
        if dc.is_null() {
            return Err(win_error("无法测量确认文字"));
        }
        let previous = SelectObject(dc, font);
        let mut rect = Rect {
            right: width,
            ..Rect::default()
        };
        let height = DrawTextW(dc, text.as_ptr(), -1, &mut rect, 0x0400 | 0x0010 | 0x0800);
        SelectObject(dc, previous);
        ReleaseDC(window, dc);
        if height <= 0 {
            Err("无法计算确认文字的显示高度。".into())
        } else {
            Ok(rect.bottom - rect.top)
        }
    }

    unsafe fn place(
        window: Handle,
        id: usize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<()> {
        if SetWindowPos(
            GetDlgItem(window, id as i32),
            null_mut(),
            x,
            y,
            width,
            height,
            0x0014,
        ) == 0
        {
            Err(win_error("无法排列确认弹窗内容"))
        } else {
            Ok(())
        }
    }

    unsafe fn paint(window: Handle, state: &State) -> Result<()> {
        let mut ps: PaintStruct = std::mem::zeroed();
        let dc = BeginPaint(window, &mut ps);
        struct PaintGuard(Handle, PaintStruct);
        impl Drop for PaintGuard {
            fn drop(&mut self) {
                unsafe {
                    EndPaint(self.0, &self.1);
                }
            }
        }
        let _paint = PaintGuard(window, ps);
        if dc.is_null() {
            return Err(win_error("无法绘制参考图片"));
        }
        let mut client = Rect::default();
        GetClientRect(window, &mut client);
        FillRect(dc, &client, GetStockObject(0));
        let rect = state.image_rect.get();
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return Ok(());
        }
        let mut graphics = null_mut();
        let api = state.image.1;
        let mut status = (api.create_graphics)(dc, &mut graphics);
        if status == 0 && !graphics.is_null() {
            let graphics = Graphics(graphics, api);
            (api.interpolation)(graphics.0, 7); // HighQualityBicubic
            status = (api.draw_image)(
                graphics.0,
                state.image.0,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            );
        } else {
            if !graphics.is_null() {
                (api.delete_graphics)(graphics);
            }
            if status == 0 {
                status = 1;
            }
        }
        if status != 0 {
            Err(format!(
                "无法绘制参考图片：{}（GDI+ 错误 {status}）。",
                state.image_path
            ))
        } else {
            Ok(())
        }
    }
}
