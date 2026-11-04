//! A shell-thread menu: opening it never initialises gpui, Direct3D or DirectWrite.
//! Win32 owns tracking, keyboard navigation and dismissal; GDI preserves the VGUI palette.

use anyhow::{Result, ensure};
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_CHECKED, ODS_SELECTED, ODT_MENU,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PWSTR, w};

pub(crate) const CAPTURE: u32 = 0x7001;
pub(crate) const SETTINGS: u32 = 0x7002;
pub(crate) const PAUSE: u32 = 0x7003;
pub(crate) const QUIT: u32 = 0x7004;
const TITLE: u32 = 0x7005;
const RULE_ONE: u32 = 0x7006;
const RULE_TWO: u32 = 0x7007;

pub(crate) struct TrayMenu {
    pub handle: HMENU,
    background: HBRUSH,
    regular: HFONT,
    bold: HFONT,
    title: HFONT,
    scale: f32,
    opened: std::cell::Cell<Option<std::time::Instant>>,
}

impl TrayMenu {
    pub fn new() -> Result<Self> {
        // SAFETY: resources belong to the shell thread and are released by Drop, including
        // partially constructed menus when inserting or styling an item fails.
        unsafe {
            let handle = CreatePopupMenu()?;
            let scale = GetDpiForSystem() as f32 / 96.0;
            let menu = Self {
                handle,
                scale,
                background: CreateSolidBrush(color(0x666666)),
                regular: font(scale, 12, 300),
                bold: font(scale, 12, 700),
                title: font(scale, 10, 700),
                opened: std::cell::Cell::new(None),
            };
            ensure!(
                !menu.background.is_invalid()
                    && !menu.regular.is_invalid()
                    && !menu.bold.is_invalid()
                    && !menu.title.is_invalid(),
                "could not create tray menu drawing resources"
            );
            SetMenuInfo(
                handle,
                &MENUINFO {
                    cbSize: std::mem::size_of::<MENUINFO>() as u32,
                    fMask: MIM_BACKGROUND | MIM_STYLE,
                    dwStyle: MNS_NOCHECK,
                    hbrBack: menu.background,
                    ..Default::default()
                },
            )?;
            for (id, label, disabled) in [
                (TITLE, "RESHOT", true),
                (CAPTURE, "&Capture", false),
                (SETTINGS, "&Settings…", false),
                (RULE_ONE, "", true),
                (PAUSE, "&Pause hotkey", false),
                (RULE_TWO, "", true),
                (QUIT, "&Quit", false),
            ] {
                // Retain real labels as well as owner drawing, for menu accessibility.
                let mut label: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
                InsertMenuItemW(
                    handle,
                    u32::MAX,
                    true,
                    &MENUITEMINFOW {
                        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_ID | MIIM_FTYPE | MIIM_STATE | MIIM_STRING,
                        fType: MFT_OWNERDRAW,
                        fState: if disabled { MFS_DISABLED } else { MFS_ENABLED },
                        wID: id,
                        dwTypeData: PWSTR(label.as_mut_ptr()),
                        ..Default::default()
                    },
                )?;
            }
            Ok(menu)
        }
    }

    pub fn prepare(&self, paused: bool, opened: std::time::Instant) {
        self.opened.set(Some(opened));
        unsafe {
            CheckMenuItem(
                self.handle,
                PAUSE,
                MF_BYCOMMAND.0 | if paused { MF_CHECKED.0 } else { MF_UNCHECKED.0 },
            );
        }
    }

    pub fn measure(&self, item: &mut MEASUREITEMSTRUCT) -> bool {
        if item.CtlType != ODT_MENU {
            return false;
        }
        item.itemWidth = self.px(180) as u32;
        item.itemHeight = self.px(match item.itemID {
            TITLE => 26,
            RULE_ONE => 12,
            RULE_TWO => 8,
            QUIT => 32,
            _ => 28,
        }) as u32;
        true
    }

    pub fn draw(&self, item: &DRAWITEMSTRUCT) -> bool {
        if item.CtlType != ODT_MENU {
            return false;
        }
        let dc = item.hDC;
        let r = item.rcItem;
        // SAFETY: Windows lends this DC for WM_DRAWITEM. RestoreDC prevents leaking font,
        // colour and background state to the next item. Every temporary brush is deleted.
        unsafe {
            let saved = SaveDC(dc);
            let _ = FillRect(dc, &r, self.background);
            let mut inner = RECT {
                left: r.left + self.px(6),
                right: r.right - self.px(6),
                top: r.top + self.px(2),
                bottom: r.bottom - self.px(2),
            };
            let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
            if matches!(item.itemID, RULE_ONE | RULE_TWO) {
                let y = (r.top + r.bottom) / 2;
                fill(
                    dc,
                    RECT {
                        top: y,
                        bottom: y + self.px(1),
                        ..inner
                    },
                    0x434343,
                );
                fill(
                    dc,
                    RECT {
                        top: y + self.px(1),
                        bottom: y + self.px(2),
                        ..inner
                    },
                    0xBBBABA,
                );
            } else {
                if item.itemID == QUIT {
                    inner.bottom -= self.px(4);
                }
                if item.itemID != TITLE {
                    if item.itemID != PAUSE || selected {
                        fill(dc, inner, if selected { 0x3C9898 } else { 0x7B7B7B });
                    }
                    if item.itemID != PAUSE {
                        bevel(dc, inner, self.px(1), 0xBBBBBB, 0x454545);
                    }
                }
                let (label, font) = match item.itemID {
                    TITLE => ("RESHOT", self.title),
                    CAPTURE => ("&Capture", self.bold),
                    SETTINGS => ("&Settings…", self.regular),
                    PAUSE => ("&Pause hotkey", self.regular),
                    QUIT => ("&Quit", self.regular),
                    _ => ("", self.regular),
                };
                if item.itemID == PAUSE {
                    let x = inner.left + self.px(6);
                    let y = (inner.top + inner.bottom - self.px(12)) / 2;
                    let check = RECT {
                        left: x,
                        top: y,
                        right: x + self.px(12),
                        bottom: y + self.px(12),
                    };
                    fill(dc, check, 0x555555);
                    bevel(dc, check, self.px(1), 0x424343, 0xBABABA);
                    if item.itemState.0 & ODS_CHECKED.0 != 0 {
                        let mut mark = "✓".encode_utf16().collect::<Vec<_>>();
                        SelectObject(dc, self.bold.into());
                        SetTextColor(dc, color(0xFFFFFF));
                        SetBkMode(dc, TRANSPARENT);
                        let mut check = check;
                        DrawTextW(
                            dc,
                            &mut mark,
                            &mut check,
                            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                        );
                    }
                    inner.left += self.px(26);
                } else {
                    inner.left += self.px(if item.itemID == TITLE { 2 } else { 6 });
                }
                SelectObject(dc, font.into());
                SetTextColor(dc, color(0xFFFFFF));
                SetBkMode(dc, TRANSPARENT);
                let mut label: Vec<u16> = label.encode_utf16().collect();
                DrawTextW(dc, &mut label, &mut inner, DT_VCENTER | DT_SINGLELINE);
            }
            let _ = RestoreDC(dc, saved);
        }
        if let Some(opened) = self.opened.take() {
            log::info!(
                "Tray menu: first paint in {:.1} ms.",
                opened.elapsed().as_secs_f64() * 1000.0
            );
        }
        true
    }

    fn px(&self, value: i32) -> i32 {
        (value as f32 * self.scale).round() as i32
    }
}

impl Drop for TrayMenu {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.handle);
            let _ = DeleteObject(self.background.into());
            let _ = DeleteObject(self.regular.into());
            let _ = DeleteObject(self.bold.into());
            let _ = DeleteObject(self.title.into());
        }
    }
}

fn font(scale: f32, size: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -(size as f32 * scale).round() as i32,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Segoe UI"),
        )
    }
}

fn color(rgb: u32) -> COLORREF {
    COLORREF((rgb >> 16) | (rgb & 0x00ff00) | ((rgb & 0xff) << 16))
}

fn fill(dc: HDC, rect: RECT, rgb: u32) {
    unsafe {
        let brush = CreateSolidBrush(color(rgb));
        let _ = FillRect(dc, &rect, brush);
        let _ = DeleteObject(brush.into());
    }
}

fn bevel(dc: HDC, r: RECT, width: i32, light: u32, dark: u32) {
    fill(
        dc,
        RECT {
            bottom: r.top + width,
            ..r
        },
        light,
    );
    fill(
        dc,
        RECT {
            right: r.left + width,
            ..r
        },
        light,
    );
    fill(
        dc,
        RECT {
            top: r.bottom - width,
            ..r
        },
        dark,
    );
    fill(
        dc,
        RECT {
            left: r.right - width,
            ..r
        },
        dark,
    );
}
