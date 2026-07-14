use anyhow::Result;
use std::{collections::HashMap, os::windows::ffi::OsStrExt, path::PathBuf};
use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        Graphics::{
            Direct2D::{
                Common::{
                    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
                },
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_DRAW_TEXT_OPTIONS_CLIP,
                D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
                D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
                D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_ROUNDED_RECT, D2D1CreateFactory,
                ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
            },
            DirectWrite::{
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
                DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Imaging::{
                CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
                WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom,
            },
        },
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        UI::{
            Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW},
            WindowsAndMessaging::{DestroyIcon, GetClientRect},
        },
    },
    core::w,
};

#[derive(Debug, Clone)]
pub struct DockVisual {
    pub label: String,
    pub running: bool,
    pub icon_path: Option<PathBuf>,
}

struct Surface {
    target: ID2D1HwndRenderTarget,
    foreground: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    panel: ID2D1SolidColorBrush,
    icons: HashMap<PathBuf, windows::Win32::Graphics::Direct2D::ID2D1Bitmap>,
}

pub struct Renderer {
    factory: ID2D1Factory,
    body: IDWriteTextFormat,
    small: IDWriteTextFormat,
    surfaces: HashMap<isize, Surface>,
    wic: IWICImagingFactory,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let text_factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let body = text_factory.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                14.0,
                w!("en-us"),
            )?;
            body.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            body.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let small = text_factory.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.0,
                w!("en-us"),
            )?;
            small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            small.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            Ok(Self {
                factory,
                body,
                small,
                surfaces: HashMap::new(),
                wic,
            })
        }
    }

    fn surface(&mut self, hwnd: HWND) -> Result<&mut Surface> {
        let key = hwnd.0 as isize;
        if !self.surfaces.contains_key(&key) {
            let mut rect = RECT::default();
            unsafe {
                GetClientRect(hwnd, &mut rect)?;
            }
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 0.0,
                dpiY: 0.0,
                usage: Default::default(),
                minLevel: Default::default(),
            };
            let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U {
                    width: (rect.right - rect.left).max(1) as u32,
                    height: (rect.bottom - rect.top).max(1) as u32,
                },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            unsafe {
                let target = self.factory.CreateHwndRenderTarget(&props, &hwnd_props)?;
                let foreground =
                    target.CreateSolidColorBrush(&color(0xf5, 0xf7, 0xff, 1.0), None)?;
                let accent = target.CreateSolidColorBrush(&color(0x70, 0xa8, 0xff, 1.0), None)?;
                let panel = target.CreateSolidColorBrush(&color(0x12, 0x17, 0x22, 1.0), None)?;
                self.surfaces.insert(
                    key,
                    Surface {
                        target,
                        foreground,
                        accent,
                        panel,
                        icons: HashMap::new(),
                    },
                );
            }
        }
        Ok(self.surfaces.get_mut(&key).expect("surface inserted"))
    }

    pub fn resize(&mut self, hwnd: HWND, width: u32, height: u32) {
        if let Some(surface) = self.surfaces.get(&(hwnd.0 as isize)) {
            unsafe {
                let _ = surface.target.Resize(&D2D_SIZE_U {
                    width: width.max(1),
                    height: height.max(1),
                });
            }
        }
    }

    pub fn forget(&mut self, hwnd: HWND) {
        self.surfaces.remove(&(hwnd.0 as isize));
    }

    pub fn paint_top_bar(&mut self, hwnd: HWND, left: &str, right: &str) -> Result<()> {
        let body = self.body.clone();
        let surface = self.surface(hwnd)?;
        let size = unsafe { surface.target.GetSize() };
        unsafe {
            surface.target.BeginDraw();
            surface.target.Clear(Some(&color(0x08, 0x0c, 0x14, 1.0)));
            let left_text: Vec<u16> = format!("  ◉   YumeDock     {left}")
                .encode_utf16()
                .collect();
            let right_text: Vec<u16> = format!("{right}   ").encode_utf16().collect();
            surface.target.DrawText(
                &left_text,
                &body,
                &D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: size.width * 0.52,
                    bottom: size.height,
                },
                &surface.foreground,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            body.SetTextAlignment(
                windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_TRAILING,
            )?;
            surface.target.DrawText(
                &right_text,
                &body,
                &D2D_RECT_F {
                    left: size.width * 0.45,
                    top: 0.0,
                    right: size.width,
                    bottom: size.height,
                },
                &surface.foreground,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            body.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            surface.target.EndDraw(None, None)?;
        }
        Ok(())
    }

    pub fn paint_dock(
        &mut self,
        hwnd: HWND,
        items: &[DockVisual],
        hover: Option<usize>,
        icon_size: f32,
        magnification: f32,
    ) -> Result<()> {
        let small = self.small.clone();
        let wic = self.wic.clone();
        let surface = self.surface(hwnd)?;
        let size = unsafe { surface.target.GetSize() };
        let gap = 8.0;
        let total = items.len() as f32 * (icon_size + gap) + 24.0;
        let start = ((size.width - total) / 2.0).max(8.0) + 12.0;
        unsafe {
            surface.target.BeginDraw();
            surface.target.Clear(Some(&color(0x08, 0x0c, 0x14, 1.0)));
            surface.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: (start - 12.0).max(4.0),
                        top: 5.0,
                        right: (start + total - 12.0).min(size.width - 4.0),
                        bottom: size.height - 5.0,
                    },
                    radiusX: 18.0,
                    radiusY: 18.0,
                },
                &surface.panel,
            );
            for (index, item) in items.iter().enumerate() {
                let distance = hover.map(|h| h.abs_diff(index) as f32).unwrap_or(4.0);
                let influence = (1.0 - distance / 2.2).clamp(0.0, 1.0);
                let scale = 1.0 + (magnification - 1.0) * influence;
                let side = icon_size * scale;
                let center_x = start + index as f32 * (icon_size + gap) + icon_size / 2.0;
                let bottom = size.height - 15.0;
                let rect = D2D_RECT_F {
                    left: center_x - side / 2.0,
                    top: bottom - side,
                    right: center_x + side / 2.0,
                    bottom,
                };
                let bitmap = item.icon_path.as_ref().and_then(|path| {
                    if !surface.icons.contains_key(path)
                        && let Some(bitmap) = load_icon_bitmap(&wic, &surface.target, path)
                    {
                        surface.icons.insert(path.clone(), bitmap);
                    }
                    surface.icons.get(path).cloned()
                });
                if let Some(bitmap) = bitmap {
                    surface.target.DrawBitmap(
                        &bitmap,
                        Some(&rect),
                        1.0,
                        D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                        None,
                    );
                } else {
                    let brush = surface
                        .target
                        .CreateSolidColorBrush(&color(46, 54, 70, 1.0), None)?;
                    surface.target.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect,
                            radiusX: side * 0.23,
                            radiusY: side * 0.23,
                        },
                        &brush,
                    );
                    let initial: Vec<u16> = item
                        .label
                        .chars()
                        .next()
                        .unwrap_or('•')
                        .to_uppercase()
                        .to_string()
                        .encode_utf16()
                        .collect();
                    surface.target.DrawText(
                        &initial,
                        &small,
                        &rect,
                        &surface.foreground,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
                if item.running {
                    let dot = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: center_x - 2.5,
                            top: size.height - 10.0,
                            right: center_x + 2.5,
                            bottom: size.height - 5.0,
                        },
                        radiusX: 2.5,
                        radiusY: 2.5,
                    };
                    surface.target.FillRoundedRectangle(&dot, &surface.accent);
                }
            }
            surface.target.EndDraw(None, None)?;
        }
        Ok(())
    }
}

fn load_icon_bitmap(
    wic: &IWICImagingFactory,
    target: &ID2D1HwndRenderTarget,
    path: &std::path::Path,
) -> Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut info = SHFILEINFOW::default();
    unsafe {
        let result = SHGetFileInfoW(
            windows::core::PCWSTR(wide.as_ptr()),
            Default::default(),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if result == 0 || info.hIcon.is_invalid() {
            return None;
        }
        let source = wic.CreateBitmapFromHICON(info.hIcon).ok();
        let _ = DestroyIcon(info.hIcon);
        let source = source?;
        let converter = wic.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &source,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None::<&windows::Win32::Graphics::Imaging::IWICPalette>,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;
        target.CreateBitmapFromWicBitmap(&converter, None).ok()
    }
}

fn color(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}
