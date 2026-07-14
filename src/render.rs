use anyhow::{Context, Result};
use std::{collections::HashMap, os::windows::ffi::OsStrExt, path::PathBuf};
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND, RECT, SIZE},
        Graphics::{
            Direct2D::{
                Common::{
                    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D1_FILL_MODE_WINDING,
                    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
                },
                D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
                D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
                D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                D2D1_ROUNDED_RECT, D2D1CreateDevice, ID2D1Bitmap1, ID2D1Brush, ID2D1DeviceContext,
                ID2D1Factory, ID2D1Image, ID2D1PathGeometry, ID2D1SolidColorBrush,
            },
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice,
                ID3D11Device, ID3D11DeviceContext,
            },
            DirectComposition::{
                DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget,
                IDCompositionVisual,
            },
            DirectWrite::{
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
                DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
                DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
            },
            Dxgi::{
                Common::{
                    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
                },
                DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
                DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice,
                IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
            },
            Gdi::{DeleteObject, HPALETTE},
            Imaging::{
                CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
                WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom,
                WICBitmapUsePremultipliedAlpha,
            },
        },
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ},
        UI::{
            HiDpi::GetDpiForWindow,
            Shell::{
                ExtractIconExW, IShellItemImageFactory, IShellLinkW, SHCreateItemFromParsingName,
                SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGSI_ICON, SHGSI_ICONLOCATION,
                SHGSI_LARGEICON, SHGetFileInfoW, SHGetStockIconInfo, SHSTOCKICONINFO,
                SIID_APPLICATION, SIID_FOLDER, SIID_RECYCLER, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
                ShellLink,
            },
            WindowsAndMessaging::{DestroyIcon, GetClientRect, HICON, PrivateExtractIconsW},
        },
    },
    core::{Interface, PCWSTR, w},
};

const ICON_GAP: f32 = 7.0;
const SECTION_GAP: f32 = 13.0;

/// Horizontal padding inside a menu-bar segment's hit/pill rect, in DIPs.
const BAR_SEGMENT_PAD_X: f32 = 10.0;
/// Vertical inset of the pill within the bar.
const BAR_PILL_INSET_Y: f32 = 5.0;
/// Gap between adjacent right-side menu-bar segments.
const BAR_SEGMENT_GAP: f32 = 4.0;
/// Fixed widths of the icon-only right-side segments (DIPs).
const BAR_SEGMENT_ICON_WIDTH: f32 = 26.0;
/// Width of the clock segment (date + time), wide enough for "Wed 14 Jul  10:04 AM".
const BAR_SEGMENT_CLOCK_WIDTH: f32 = 168.0;
/// Reserved left-cluster width: mark + bold app name.
const BAR_LEFT_MARK_RADIUS: f32 = 4.5;
const BAR_LEFT_MARK_OFFSET: f32 = 14.0;
const BAR_LEFT_GAP_AFTER_MARK: f32 = 9.0;

/// A discrete, hit-testable region of the macOS-style menu bar.
///
/// Ordering matches left-to-right visual order; the `Logo` and `App` segments
/// form the fixed left cluster, the rest are right-aligned status segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TopBarSegment {
    Logo,
    App,
    Network,
    Volume,
    Battery,
    Clock,
}

impl TopBarSegment {
    /// Discriminant used to (de)serialize hover state through the existing
    /// `Option<usize>` hover map shared with the dock.
    pub fn encode(self) -> usize {
        match self {
            TopBarSegment::Logo => 0,
            TopBarSegment::App => 1,
            TopBarSegment::Network => 2,
            TopBarSegment::Volume => 3,
            TopBarSegment::Battery => 4,
            TopBarSegment::Clock => 5,
        }
    }

    pub fn decode(value: usize) -> Option<Self> {
        match value {
            0 => Some(Self::Logo),
            1 => Some(Self::App),
            2 => Some(Self::Network),
            3 => Some(Self::Volume),
            4 => Some(Self::Battery),
            5 => Some(Self::Clock),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TopBarGeometry {
    /// Segments with their pill rects, in left-to-right order.
    pub segments: Vec<(TopBarSegment, D2D_RECT_F)>,
}

impl TopBarGeometry {
    pub fn find(&self, segment: TopBarSegment) -> Option<&D2D_RECT_F> {
        self.segments
            .iter()
            .find(|(seg, _)| *seg == segment)
            .map(|(_, rect)| rect)
    }
}

/// Status payload rendered into the right side of the bar. Mirrors
/// `crate::status::SystemStatus` but owned here to keep the render module
/// decoupled from the Win32 status reader.
#[derive(Debug, Clone, Copy, Default)]
pub struct TopBarStatus {
    pub battery_percent: Option<u8>,
    pub charging: bool,
    pub network_online: bool,
    pub volume_percent: Option<u8>,
    pub muted: bool,
}

/// Which status segments to render, lifted from `TopBarConfig` so this module
/// stays free of the config types.
#[derive(Debug, Clone, Copy, Default)]
pub struct TopBarSegmentFlags {
    pub show_network: bool,
    pub show_volume: bool,
    pub show_battery: bool,
}

#[derive(Debug, Clone)]
pub struct DockVisual {
    pub label: String,
    pub running: bool,
    pub icon_path: Option<PathBuf>,
    pub fallback_icon_path: Option<PathBuf>,
    pub separator_before: bool,
    pub recycle_bin: bool,
    pub folder: bool,
}

#[derive(Clone, Copy)]
pub struct DockHover {
    pub index: usize,
    pub x: f32,
}

#[derive(Clone, Copy)]
pub struct DockRenderState {
    pub hover: Option<DockHover>,
    pub magnification: f32,
    pub bounce: Option<DockBounce>,
    pub hide_progress: f32,
    pub dragging: Option<usize>,
    pub pressed: Option<usize>,
    pub reorder: Option<(usize, usize, f32)>,
}

#[derive(Clone, Copy)]
pub struct DockBounce {
    pub item: usize,
    pub offset: f32,
    pub scale_x: f32,
    pub scale_y: f32,
}

#[derive(Clone, Copy)]
struct IconGeometry {
    center: f32,
    side: f32,
    left: f32,
}

struct DockGeometry {
    icons: Vec<IconGeometry>,
    content_width: f32,
}

struct Surface {
    context: ID2D1DeviceContext,
    swap_chain: IDXGISwapChain1,
    target_bitmap: ID2D1Bitmap1,
    _composition_target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    foreground: ID2D1SolidColorBrush,
    indicator: ID2D1SolidColorBrush,
    panel: ID2D1SolidColorBrush,
    outline: ID2D1SolidColorBrush,
    icons: HashMap<PathBuf, ID2D1Bitmap1>,
    recycle_icon: Option<ID2D1Bitmap1>,
    generic_app_icon: Option<ID2D1Bitmap1>,
    folder_icon: Option<ID2D1Bitmap1>,
}

pub struct Renderer {
    _d3d_device: ID3D11Device,
    _immediate_context: ID3D11DeviceContext,
    d2d_device: windows::Win32::Graphics::Direct2D::ID2D1Device,
    factory: ID2D1Factory,
    dxgi_factory: IDXGIFactory2,
    composition: IDCompositionDevice,
    body: IDWriteTextFormat,
    bold: IDWriteTextFormat,
    small: IDWriteTextFormat,
    surfaces: HashMap<isize, Surface>,
    wic: IWICImagingFactory,
    high_contrast: bool,
    /// Cached menu-bar vector icon geometries (local origin; translated on draw).
    geo_wifi: Option<ID2D1PathGeometry>,
    geo_speaker: Option<ID2D1PathGeometry>,
    geo_speaker_muted: Option<ID2D1PathGeometry>,
    geo_battery: Option<ID2D1PathGeometry>,
    geo_bolt: Option<ID2D1PathGeometry>,
}

impl Renderer {
    pub fn new(high_contrast: bool) -> Result<Self> {
        unsafe {
            let mut d3d_device = None;
            let mut immediate_context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d_device),
                None,
                Some(&mut immediate_context),
            )?;
            let d3d_device = d3d_device.context("Direct3D device was not created")?;
            let immediate_context =
                immediate_context.context("Direct3D context was not created")?;
            let dxgi_device: IDXGIDevice = d3d_device.cast()?;
            let adapter = dxgi_device.GetAdapter()?;
            let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;
            let d2d_device = D2D1CreateDevice(&dxgi_device, None)?;
            let factory: ID2D1Factory = d2d_device.GetFactory()?;
            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;

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
            let bold = text_factory.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                14.0,
                w!("en-us"),
            )?;
            bold.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            bold.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
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
                _d3d_device: d3d_device,
                _immediate_context: immediate_context,
                d2d_device,
                factory,
                dxgi_factory,
                composition,
                body,
                bold,
                small,
                surfaces: HashMap::new(),
                wic,
                high_contrast,
                geo_wifi: None,
                geo_speaker: None,
                geo_speaker_muted: None,
                geo_battery: None,
                geo_bolt: None,
            })
        }
    }

    pub fn set_high_contrast(&mut self, high_contrast: bool) {
        self.high_contrast = high_contrast;
        self.surfaces.clear();
    }

    /// Lazily build the cached menu-bar vector icon geometries once. Each icon
    /// is authored in a compact local coordinate space and translated by the
    /// render target via a geometry transform when drawn.
    fn ensure_bar_geometries(&mut self) {
        if self.geo_wifi.is_none() {
            self.geo_wifi = build_geometry(&self.factory, wifi_figure);
        }
        if self.geo_speaker.is_none() {
            self.geo_speaker = build_geometry(&self.factory, speaker_figure);
        }
        if self.geo_speaker_muted.is_none() {
            self.geo_speaker_muted = build_geometry(&self.factory, speaker_muted_figure);
        }
        if self.geo_battery.is_none() {
            self.geo_battery = build_geometry(&self.factory, battery_outline_figure);
        }
        if self.geo_bolt.is_none() {
            self.geo_bolt = build_geometry(&self.factory, bolt_figure);
        }
    }

    fn surface(&mut self, hwnd: HWND) -> Result<&mut Surface> {
        let key = hwnd.0 as isize;
        if !self.surfaces.contains_key(&key) {
            let mut rect = RECT::default();
            unsafe { GetClientRect(hwnd, &mut rect)? };
            let width = (rect.right - rect.left).max(1) as u32;
            let height = (rect.bottom - rect.top).max(1) as u32;
            let surface = unsafe { self.create_surface(hwnd, width, height)? };
            self.surfaces.insert(key, surface);
        }
        Ok(self.surfaces.get_mut(&key).expect("surface inserted"))
    }

    unsafe fn create_surface(&self, hwnd: HWND, width: u32, height: u32) -> Result<Surface> {
        let context = unsafe {
            self.d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?
        };
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let swap_chain = unsafe {
            self.dxgi_factory.CreateSwapChainForComposition(
                &self._d3d_device,
                &desc,
                None::<&windows::Win32::Graphics::Dxgi::IDXGIOutput>,
            )?
        };
        let dpi = unsafe { GetDpiForWindow(hwnd).max(96) as f32 };
        let target_bitmap = unsafe { create_target_bitmap(&context, &swap_chain, dpi)? };
        unsafe { context.SetTarget(&target_bitmap) };
        unsafe { context.SetDpi(dpi, dpi) };

        let composition_target = unsafe { self.composition.CreateTargetForHwnd(hwnd, true)? };
        let visual = unsafe { self.composition.CreateVisual()? };
        unsafe {
            visual.SetContent(&swap_chain)?;
            composition_target.SetRoot(&visual)?;
            self.composition.Commit()?;
        }
        let foreground =
            unsafe { context.CreateSolidColorBrush(&color(0xf5, 0xf7, 0xfb, 1.0), None)? };
        let indicator =
            unsafe { context.CreateSolidColorBrush(&color(0xf4, 0xf6, 0xfa, 0.92), None)? };
        let panel = unsafe {
            context.CreateSolidColorBrush(
                &color(
                    0x20,
                    0x26,
                    0x2f,
                    if self.high_contrast { 1.0 } else { 0.46 },
                ),
                None,
            )?
        };
        let outline =
            unsafe { context.CreateSolidColorBrush(&color(0xdf, 0xe5, 0xee, 0.28), None)? };
        Ok(Surface {
            context,
            swap_chain,
            target_bitmap,
            _composition_target: composition_target,
            _visual: visual,
            foreground,
            indicator,
            panel,
            outline,
            icons: HashMap::new(),
            recycle_icon: None,
            generic_app_icon: None,
            folder_icon: None,
        })
    }

    pub fn resize(&mut self, hwnd: HWND, width: u32, height: u32) {
        let Some(surface) = self.surfaces.get_mut(&(hwnd.0 as isize)) else {
            return;
        };
        unsafe {
            let dpi = GetDpiForWindow(hwnd).max(96) as f32;
            surface.context.SetTarget(None::<&ID2D1Image>);
            if surface
                .swap_chain
                .ResizeBuffers(
                    2,
                    width.max(1),
                    height.max(1),
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .is_ok()
                && let Ok(bitmap) = create_target_bitmap(&surface.context, &surface.swap_chain, dpi)
            {
                surface.context.SetTarget(&bitmap);
                surface.context.SetDpi(dpi, dpi);
                surface.target_bitmap = bitmap;
                surface.icons.clear();
                surface.recycle_icon = None;
                surface.generic_app_icon = None;
                surface.folder_icon = None;
            }
        }
    }

    pub fn forget(&mut self, hwnd: HWND) {
        self.surfaces.remove(&(hwnd.0 as isize));
    }

    pub fn paint_top_bar(
        &mut self,
        hwnd: HWND,
        app_name: &str,
        clock: &str,
        date: &str,
        status: TopBarStatus,
        flags: TopBarSegmentFlags,
        hover: Option<TopBarSegment>,
    ) -> Result<()> {
        let high_contrast = self.high_contrast;
        let body = self.body.clone();
        let bold = self.bold.clone();
        let small = self.small.clone();
        self.ensure_bar_geometries();
        let geo_wifi = self.geo_wifi.clone();
        let geo_speaker = self.geo_speaker.clone();
        let geo_speaker_muted = self.geo_speaker_muted.clone();
        let geo_battery = self.geo_battery.clone();
        let geo_bolt = self.geo_bolt.clone();
        let surface = self.surface(hwnd)?;
        let size = unsafe { surface.context.GetSize() };
        let geometry = top_bar_geometry(
            size.width,
            size.height,
            app_name,
            status,
            flags,
        );
        unsafe {
            surface.context.BeginDraw();
            // Bar backdrop: slightly deeper than the dock for separation.
            surface.context.Clear(Some(&color(
                0x0a,
                0x0e,
                0x15,
                if high_contrast { 1.0 } else { 0.55 },
            )));

            let pill_fill = surface
                .context
                .CreateSolidColorBrush(&color(0xf5, 0xf7, 0xfa, 0.14), None)?;
            let dim = surface
                .context
                .CreateSolidColorBrush(&color(0xf5, 0xf7, 0xfa, 0.45), None)?;
            let charge = surface
                .context
                .CreateSolidColorBrush(&color(0x6c, 0xe0, 0x8a, 1.0), None)?;
            let low = surface
                .context
                .CreateSolidColorBrush(&color(0xff, 0x7a, 0x6b, 1.0), None)?;

            // --- Left cluster: neutral mark + bold app name. ---
            let mark_cx = BAR_LEFT_MARK_OFFSET + BAR_LEFT_MARK_RADIUS;
            let mark_cy = size.height / 2.0;
            surface.context.FillEllipse(
                &D2D1_ELLIPSE {
                    point: vec2(mark_cx, mark_cy),
                    radiusX: BAR_LEFT_MARK_RADIUS,
                    radiusY: BAR_LEFT_MARK_RADIUS,
                },
                &surface.foreground,
            );
            if hover == Some(TopBarSegment::Logo) {
                surface.context.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: vec2(mark_cx, mark_cy),
                        radiusX: BAR_LEFT_MARK_RADIUS + 4.0,
                        radiusY: BAR_LEFT_MARK_RADIUS + 4.0,
                    },
                    &dim,
                );
                surface.context.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: vec2(mark_cx, mark_cy),
                        radiusX: BAR_LEFT_MARK_RADIUS,
                        radiusY: BAR_LEFT_MARK_RADIUS,
                    },
                    &surface.foreground,
                );
            }
            if let Some(rect) = geometry.find(TopBarSegment::App) {
                let app_text: Vec<u16> = app_name.encode_utf16().collect();
                surface.context.DrawText(
                    &app_text,
                    &bold,
                    rect,
                    &surface.foreground,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // --- Right segments. ---
            for (segment, rect) in &geometry.segments {
                match segment {
                    TopBarSegment::Logo | TopBarSegment::App => continue,
                    TopBarSegment::Clock => {
                        if hover == Some(TopBarSegment::Clock) {
                            fill_pill(&surface.context, rect, &pill_fill);
                        }
                        // Date (small) leading in the left half, time (body)
                        // trailing in the right half, with a clean gap so they
                        // never overlap regardless of locale string width.
                        let date_text: Vec<u16> = date.encode_utf16().collect();
                        let clock_text: Vec<u16> = clock.encode_utf16().collect();
                        let gap = 8.0;
                        let mid = rect.left + rect.width() / 2.0;
                        small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
                        surface.context.DrawText(
                            &date_text,
                            &small,
                            &D2D_RECT_F {
                                left: rect.left + BAR_SEGMENT_PAD_X,
                                top: 0.0,
                                right: mid - gap / 2.0,
                                bottom: size.height,
                            },
                            &dim,
                            D2D1_DRAW_TEXT_OPTIONS_CLIP,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                        body.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
                        surface.context.DrawText(
                            &clock_text,
                            &body,
                            &D2D_RECT_F {
                                left: mid + gap / 2.0,
                                top: 0.0,
                                right: rect.right - BAR_SEGMENT_PAD_X,
                                bottom: size.height,
                            },
                            &surface.foreground,
                            D2D1_DRAW_TEXT_OPTIONS_CLIP,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                    TopBarSegment::Network => {
                        if hover == Some(TopBarSegment::Network) {
                            fill_pill(&surface.context, rect, &pill_fill);
                        }
                        if let Some(geo) = geo_wifi.as_ref() {
                            draw_icon(&surface.context, geo, rect, 16.0, 18.0, if status.network_online { &surface.foreground } else { &dim });
                        }
                        if !status.network_online {
                            // Strike-through line over the icon: shape conveys "off",
                            // not color alone.
                            surface.context.FillRectangle(
                                &D2D_RECT_F {
                                    left: rect.center_x() - 9.0,
                                    top: rect.center_y() - 0.5,
                                    right: rect.center_x() + 9.0,
                                    bottom: rect.center_y() + 0.5,
                                },
                                &dim,
                            );
                        }
                    }
                    TopBarSegment::Volume => {
                        if hover == Some(TopBarSegment::Volume) {
                            fill_pill(&surface.context, rect, &pill_fill);
                        }
                        let muted = status.muted;
                        let geo = if muted { geo_speaker_muted.as_ref() } else { geo_speaker.as_ref() };
                        if let Some(geo) = geo {
                            let brush = if muted { &dim } else { &surface.foreground };
                            draw_icon(&surface.context, geo, rect, 14.0, 14.0, brush);
                        }
                    }
                    TopBarSegment::Battery => {
                        if hover == Some(TopBarSegment::Battery) {
                            fill_pill(&surface.context, rect, &pill_fill);
                        }
                        let Some(percent) = status.battery_percent else { continue };
                        // Outline + nub rendered via the cached geometry (translated),
                        // then a proportional fill rect for the charge level.
                        let cell_w = 22.0;
                        let cell_h = 11.0;
                        let cx = rect.center_x();
                        let cy = rect.center_y();
                        let outline = D2D_RECT_F {
                            left: cx - cell_w / 2.0,
                            top: cy - cell_h / 2.0,
                            right: cx + cell_w / 2.0,
                            bottom: cy + cell_h / 2.0,
                        };
                        // Battery nub (positive terminal) on the right.
                        surface.context.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: outline.right - 0.5,
                                    top: cy - 2.5,
                                    right: outline.right + 2.0,
                                    bottom: cy + 2.5,
                                },
                                radiusX: 1.0,
                                radiusY: 1.0,
                            },
                            &surface.foreground,
                        );
                        let _ = geo_battery.as_ref(); // outline geometry reserved for future stroke
                        surface.context.DrawRoundedRectangle(
                            &D2D1_ROUNDED_RECT { rect: outline, radiusX: 3.0, radiusY: 3.0 },
                            &surface.foreground,
                            1.2,
                            None::<&windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle>,
                        );
                        // Interior charge fill.
                        let level = (percent as f32 / 100.0).clamp(0.0, 1.0);
                        let inner_pad = 2.0;
                        let fill_w = (outline.width() - inner_pad * 2.0).max(0.0) * level;
                        let fill_brush = if percent <= 20 { &low } else { &charge };
                        surface.context.FillRectangle(
                            &D2D_RECT_F {
                                left: outline.left + inner_pad,
                                top: outline.top + inner_pad,
                                right: outline.left + inner_pad + fill_w,
                                bottom: outline.bottom - inner_pad,
                            },
                            fill_brush,
                        );
                        if status.charging {
                            if let Some(bolt) = geo_bolt.as_ref() {
                                draw_icon(&surface.context, bolt, rect, 9.0, 14.0, &surface.foreground);
                            }
                        }
                    }
                }
            }

            // Restore default alignment so other callers (paint_dock etc.) see
            // the same state. Both `body` and `small` are shared COM objects --
            // a clone shares the underlying IDWriteTextFormat, so leaving
            // either in LEADING/TRAILING corrupts the dock's centered labels.
            small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            body.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            present(surface)?;
        }
        Ok(())
    }

    pub fn paint_folder_stack(
        &mut self,
        hwnd: HWND,
        title: &str,
        entries: &[(String, PathBuf)],
        hover: Option<usize>,
        footer_hover: bool,
        pointer_x: f32,
    ) -> Result<()> {
        const COLUMNS: usize = 5;
        const CELL_WIDTH: f32 = 72.0;
        const CELL_HEIGHT: f32 = 76.0;
        const PADDING: f32 = 16.0;
        const HEADER: f32 = 38.0;
        const FOOTER: f32 = 38.0;
        const ICON: f32 = 42.0;

        let high_contrast = self.high_contrast;
        let body = self.body.clone();
        let small = self.small.clone();
        let wic = self.wic.clone();
        let surface = self.surface(hwnd)?;
        let size = unsafe { surface.context.GetSize() };
        unsafe {
            surface.context.BeginDraw();
            surface.context.Clear(Some(&color(0, 0, 0, 0.0)));
            body.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;

            let panel_bottom = size.height - 10.0;
            let panel = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: 3.0,
                    top: 3.0,
                    right: size.width - 3.0,
                    bottom: panel_bottom,
                },
                radiusX: 15.0,
                radiusY: 15.0,
            };
            let shadow = surface
                .context
                .CreateSolidColorBrush(&color(0x02, 0x05, 0x0a, 0.22), None)?;
            let fill = surface.context.CreateSolidColorBrush(
                &color(0x20, 0x25, 0x2d, if high_contrast { 1.0 } else { 0.72 }),
                None,
            )?;
            let hover_fill = surface
                .context
                .CreateSolidColorBrush(&color(0xe8, 0xed, 0xf5, 0.12), None)?;
            surface.context.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: panel.rect.left - 2.0,
                        top: panel.rect.top + 1.0,
                        right: panel.rect.right + 2.0,
                        bottom: panel.rect.bottom + 3.0,
                    },
                    radiusX: 17.0,
                    radiusY: 17.0,
                },
                &shadow,
            );
            surface.context.FillRoundedRectangle(&panel, &fill);
            surface
                .context
                .DrawRoundedRectangle(&panel, &surface.outline, 1.0, None);

            let title_text: Vec<u16> = title.encode_utf16().collect();
            surface.context.DrawText(
                &title_text,
                &body,
                &D2D_RECT_F {
                    left: PADDING,
                    top: 5.0,
                    right: size.width - PADDING,
                    bottom: HEADER,
                },
                &surface.foreground,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            for (index, (label, path)) in entries.iter().enumerate() {
                let column = (index % COLUMNS) as f32;
                let row = (index / COLUMNS) as f32;
                let cell = D2D_RECT_F {
                    left: PADDING + column * CELL_WIDTH,
                    top: HEADER + row * CELL_HEIGHT,
                    right: PADDING + (column + 1.0) * CELL_WIDTH,
                    bottom: HEADER + (row + 1.0) * CELL_HEIGHT,
                };
                if hover == Some(index) {
                    surface.context.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: cell.left + 4.0,
                                top: cell.top + 2.0,
                                right: cell.right - 4.0,
                                bottom: cell.bottom - 2.0,
                            },
                            radiusX: 9.0,
                            radiusY: 9.0,
                        },
                        &hover_fill,
                    );
                }
                let icon_rect = D2D_RECT_F {
                    left: (cell.left + cell.right - ICON) / 2.0,
                    top: cell.top + 5.0,
                    right: (cell.left + cell.right + ICON) / 2.0,
                    bottom: cell.top + 5.0 + ICON,
                };
                if !surface.icons.contains_key(path)
                    && let Some(bitmap) = load_icon_bitmap(&wic, &surface.context, path)
                {
                    surface.icons.insert(path.clone(), bitmap);
                }
                if let Some(bitmap) = surface.icons.get(path) {
                    surface.context.DrawBitmap(
                        bitmap,
                        Some(&icon_rect),
                        1.0,
                        D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                        None,
                        None,
                    );
                }
                let text: Vec<u16> = label.encode_utf16().collect();
                surface.context.DrawText(
                    &text,
                    &small,
                    &D2D_RECT_F {
                        left: cell.left + 2.0,
                        top: cell.top + 49.0,
                        right: cell.right - 2.0,
                        bottom: cell.bottom - 2.0,
                    },
                    &surface.foreground,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            let footer_top = panel_bottom - FOOTER;
            surface.context.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: 9.0,
                        top: footer_top + 3.0,
                        right: size.width - 9.0,
                        bottom: panel_bottom - 5.0,
                    },
                    radiusX: 8.0,
                    radiusY: 8.0,
                },
                if footer_hover { &hover_fill } else { &fill },
            );
            let footer_text: Vec<u16> = "Open in File Explorer".encode_utf16().collect();
            surface.context.DrawText(
                &footer_text,
                &small,
                &D2D_RECT_F {
                    left: PADDING,
                    top: footer_top,
                    right: size.width - PADDING,
                    bottom: panel_bottom,
                },
                &surface.foreground,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            for row in 0..7 {
                let half_width = 6.5 - row as f32;
                let pointer_x = pointer_x.clamp(22.0, size.width - 22.0);
                surface.context.FillRectangle(
                    &D2D_RECT_F {
                        left: pointer_x - half_width,
                        top: panel_bottom + row as f32,
                        right: pointer_x + half_width,
                        bottom: panel_bottom + row as f32 + 1.0,
                    },
                    &fill,
                );
            }
            present(surface)?;
        }
        Ok(())
    }

    pub fn paint_preview(
        &mut self,
        hwnd: HWND,
        title: &str,
        close_hover: bool,
        pointer_x: f32,
    ) -> Result<()> {
        let high_contrast = self.high_contrast;
        let body = self.body.clone();
        let small = self.small.clone();
        let surface = self.surface(hwnd)?;
        let size = unsafe { surface.context.GetSize() };
        unsafe {
            surface.context.BeginDraw();
            surface.context.Clear(Some(&color(0, 0, 0, 0.0)));
            body.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;

            let panel_bottom = size.height - 10.0;
            let panel = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: 3.0,
                    top: 3.0,
                    right: size.width - 3.0,
                    bottom: panel_bottom,
                },
                radiusX: 13.0,
                radiusY: 13.0,
            };
            let shadow = surface
                .context
                .CreateSolidColorBrush(&color(0x02, 0x05, 0x0a, 0.25), None)?;
            let fill = surface.context.CreateSolidColorBrush(
                &color(0x20, 0x25, 0x2d, if high_contrast { 1.0 } else { 0.74 }),
                None,
            )?;
            surface.context.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: 1.0,
                        top: 3.0,
                        right: size.width - 1.0,
                        bottom: panel_bottom + 3.0,
                    },
                    radiusX: 15.0,
                    radiusY: 15.0,
                },
                &shadow,
            );
            surface.context.FillRoundedRectangle(&panel, &fill);
            surface
                .context
                .DrawRoundedRectangle(&panel, &surface.outline, 1.0, None);

            let close = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: 12.0,
                    top: 11.0,
                    right: 24.0,
                    bottom: 23.0,
                },
                radiusX: 6.0,
                radiusY: 6.0,
            };
            let close_fill = surface.context.CreateSolidColorBrush(
                &if close_hover {
                    color(0xff, 0x70, 0x68, 1.0)
                } else {
                    color(0xff, 0x5f, 0x57, 1.0)
                },
                None,
            )?;
            surface.context.FillRoundedRectangle(&close, &close_fill);
            if close_hover {
                let close_text: Vec<u16> = "×".encode_utf16().collect();
                let close_mark = surface
                    .context
                    .CreateSolidColorBrush(&color(0x57, 0x16, 0x12, 0.85), None)?;
                surface.context.DrawText(
                    &close_text,
                    &small,
                    &D2D_RECT_F {
                        left: 10.0,
                        top: 8.0,
                        right: 26.0,
                        bottom: 25.0,
                    },
                    &close_mark,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            let title_text: Vec<u16> = title.encode_utf16().collect();
            surface.context.DrawText(
                &title_text,
                &body,
                &D2D_RECT_F {
                    left: 36.0,
                    top: 4.0,
                    right: size.width - 16.0,
                    bottom: 34.0,
                },
                &surface.foreground,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            for row in 0..7 {
                let half_width = 6.5 - row as f32;
                let pointer_x = pointer_x.clamp(22.0, size.width - 22.0);
                surface.context.FillRectangle(
                    &D2D_RECT_F {
                        left: pointer_x - half_width,
                        top: panel_bottom + row as f32,
                        right: pointer_x + half_width,
                        bottom: panel_bottom + row as f32 + 1.0,
                    },
                    &fill,
                );
            }
            present(surface)?;
        }
        Ok(())
    }

    pub fn paint_dock(
        &mut self,
        hwnd: HWND,
        items: &[DockVisual],
        icon_size: f32,
        state: DockRenderState,
    ) -> Result<()> {
        let small = self.small.clone();
        let wic = self.wic.clone();
        let surface = self.surface(hwnd)?;
        let size = unsafe { surface.context.GetSize() };
        let base_height = icon_size + 18.0;
        let separator = items.iter().position(|item| item.separator_before);
        let geometry = dock_geometry(
            size.width,
            items.len(),
            icon_size,
            state.hover.map(|hover| hover.x),
            state.magnification,
            separator,
        );
        unsafe {
            surface.context.BeginDraw();
            surface.context.Clear(Some(&color(0, 0, 0, 0.0)));
            // Defensive: the text formats are shared COM objects that other
            // paint methods (paint_top_bar) may have left in LEADING/TRAILING
            // alignment. Reset to CENTER before drawing dock labels.
            small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            let hide_offset = state.hide_progress.clamp(0.0, 1.0) * (icon_size + 24.0);
            let shell_bottom = size.height - 8.0 + hide_offset;
            let shell_width = geometry.content_width + 32.0;
            let shell = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: ((size.width - shell_width) / 2.0).max(3.0),
                    top: shell_bottom - base_height,
                    right: ((size.width + shell_width) / 2.0).min(size.width - 3.0),
                    bottom: shell_bottom,
                },
                radiusX: 18.0,
                radiusY: 18.0,
            };
            let far_shadow = surface
                .context
                .CreateSolidColorBrush(&color(0x02, 0x05, 0x0a, 0.15), None)?;
            let near_shadow = surface
                .context
                .CreateSolidColorBrush(&color(0x02, 0x05, 0x0a, 0.18), None)?;
            surface.context.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: shell.rect.left - 4.0,
                        top: shell.rect.top + 1.0,
                        right: shell.rect.right + 4.0,
                        bottom: shell.rect.bottom + 5.0,
                    },
                    radiusX: 22.0,
                    radiusY: 22.0,
                },
                &far_shadow,
            );
            surface.context.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: shell.rect.left - 1.5,
                        top: shell.rect.top,
                        right: shell.rect.right + 1.5,
                        bottom: shell.rect.bottom + 2.0,
                    },
                    radiusX: 19.5,
                    radiusY: 19.5,
                },
                &near_shadow,
            );
            surface.context.FillRoundedRectangle(&shell, &surface.panel);
            surface
                .context
                .DrawRoundedRectangle(&shell, &surface.outline, 1.0, None);
            let top_highlight = surface
                .context
                .CreateSolidColorBrush(&color(0xf5, 0xf8, 0xfc, 0.14), None)?;
            surface.context.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: shell.rect.left + 18.0,
                        top: shell.rect.top + 0.75,
                        right: shell.rect.right - 18.0,
                        bottom: shell.rect.top + 1.5,
                    },
                    radiusX: 0.4,
                    radiusY: 0.4,
                },
                &top_highlight,
            );
            let bottom_edge = surface
                .context
                .CreateSolidColorBrush(&color(0x02, 0x05, 0x0a, 0.18), None)?;
            surface.context.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: shell.rect.left + 18.0,
                        top: shell.rect.bottom - 1.5,
                        right: shell.rect.right - 18.0,
                        bottom: shell.rect.bottom - 0.65,
                    },
                    radiusX: 0.4,
                    radiusY: 0.4,
                },
                &bottom_edge,
            );

            for (index, item) in items.iter().enumerate() {
                let icon = geometry.icons[index];
                let side = icon.side;
                let mut center_x = icon.center;
                if let Some((from, to, progress)) = state.reorder
                    && let (Some(from_icon), Some(to_icon)) =
                        (geometry.icons.get(from), geometry.icons.get(to))
                {
                    let remaining = 1.0 - progress.clamp(0.0, 1.0);
                    if index == to {
                        center_x += (from_icon.center - to_icon.center) * remaining;
                    } else if index == from {
                        center_x += (to_icon.center - from_icon.center) * remaining;
                    }
                }
                if item.separator_before {
                    let x = icon.left - (ICON_GAP + SECTION_GAP) / 2.0;
                    surface.context.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: x - 0.5,
                                top: shell_bottom - icon_size - 10.0,
                                right: x + 0.5,
                                bottom: shell_bottom - 10.0,
                            },
                            radiusX: 0.5,
                            radiusY: 0.5,
                        },
                        &surface.outline,
                    );
                }
                let bounce = state.bounce.filter(|bounce| bounce.item == index);
                let bounce_offset = bounce.map_or(0.0, |bounce| bounce.offset);
                let drag_lift = if state.dragging == Some(index) {
                    6.0
                } else {
                    0.0
                };
                let bottom =
                    shell_bottom - 9.0 - (side - icon_size) * 0.16 - bounce_offset - drag_lift;
                let draw_side = if state.pressed == Some(index) {
                    side * 0.94
                } else {
                    side
                };
                let draw_width = draw_side * bounce.map_or(1.0, |bounce| bounce.scale_x);
                let draw_height = draw_side * bounce.map_or(1.0, |bounce| bounce.scale_y);
                let rect = D2D_RECT_F {
                    left: center_x - draw_width / 2.0,
                    top: bottom - draw_height,
                    right: center_x + draw_width / 2.0,
                    bottom,
                };
                let mut bitmap = if item.recycle_bin {
                    if surface.recycle_icon.is_none() {
                        surface.recycle_icon =
                            load_stock_icon(&wic, &surface.context, SIID_RECYCLER);
                    }
                    surface.recycle_icon.clone()
                } else {
                    item.icon_path
                        .iter()
                        .chain(item.fallback_icon_path.iter())
                        .find_map(|path| {
                            if !surface.icons.contains_key(path)
                                && let Some(bitmap) = load_icon_bitmap(&wic, &surface.context, path)
                            {
                                surface.icons.insert(path.clone(), bitmap);
                            }
                            surface.icons.get(path).cloned()
                        })
                };
                if bitmap.is_none() && item.folder {
                    if surface.folder_icon.is_none() {
                        surface.folder_icon = load_stock_icon(&wic, &surface.context, SIID_FOLDER);
                    }
                    bitmap = surface.folder_icon.clone();
                }
                if bitmap.is_none() && !item.recycle_bin && !item.folder {
                    if surface.generic_app_icon.is_none() {
                        surface.generic_app_icon =
                            load_stock_icon(&wic, &surface.context, SIID_APPLICATION);
                    }
                    bitmap = surface.generic_app_icon.clone();
                }
                if let Some(bitmap) = bitmap {
                    let opacity = if state.dragging == Some(index) {
                        0.86
                    } else if state.pressed == Some(index) {
                        0.92
                    } else {
                        1.0
                    };
                    surface.context.DrawBitmap(
                        &bitmap,
                        Some(&rect),
                        opacity,
                        D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                        None,
                        None,
                    );
                } else {
                    let fallback = surface
                        .context
                        .CreateSolidColorBrush(&color(0x46, 0x4d, 0x59, 0.78), None)?;
                    surface.context.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect,
                            radiusX: draw_side * 0.22,
                            radiusY: draw_side * 0.22,
                        },
                        &fallback,
                    );
                }
                if item.running {
                    surface.context.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: center_x - 1.75,
                                top: shell_bottom - 7.5,
                                right: center_x + 1.75,
                                bottom: shell_bottom - 4.0,
                            },
                            radiusX: 1.75,
                            radiusY: 1.75,
                        },
                        &surface.indicator,
                    );
                }
            }

            if state.dragging.is_none()
                && let Some(index) = state.hover.map(|hover| hover.index)
                && let (Some(item), Some(icon)) = (items.get(index), geometry.icons.get(index))
            {
                let label_width =
                    (item.label.chars().count() as f32 * 7.2 + 24.0).clamp(58.0, 190.0);
                let label_bottom = (shell.rect.top - 8.0).min(shell_bottom - 12.0 - icon.side);
                let bubble = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: (icon.center - label_width / 2.0).max(2.0),
                        top: (label_bottom - 28.0).max(2.0),
                        right: (icon.center + label_width / 2.0).min(size.width - 2.0),
                        bottom: label_bottom,
                    },
                    radiusX: 7.0,
                    radiusY: 7.0,
                };
                let bubble_fill = surface
                    .context
                    .CreateSolidColorBrush(&color(0x20, 0x24, 0x2b, 0.92), None)?;
                surface.context.FillRoundedRectangle(&bubble, &bubble_fill);
                surface
                    .context
                    .DrawRoundedRectangle(&bubble, &surface.outline, 1.0, None);
                for row in 0..4 {
                    let half_width = 3.5 - row as f32;
                    surface.context.FillRectangle(
                        &D2D_RECT_F {
                            left: icon.center - half_width,
                            top: bubble.rect.bottom + row as f32,
                            right: icon.center + half_width,
                            bottom: bubble.rect.bottom + row as f32 + 1.0,
                        },
                        &bubble_fill,
                    );
                }
                let text: Vec<u16> = item.label.encode_utf16().collect();
                surface.context.DrawText(
                    &text,
                    &small,
                    &bubble.rect,
                    &surface.foreground,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            present(surface)?;
        }
        Ok(())
    }
}

/// Lay out the macOS-style menu bar segments.
///
/// `width`/`height` are the bar's client size in DIPs. The left cluster
/// (mark + app name) is anchored to the left edge; the status segments are
/// right-aligned and stacked right-to-left so the clock is always flush right.
/// Segments whose flag is off, or that have no data (e.g. no battery), are
/// omitted entirely.
pub fn top_bar_geometry(
    width: f32,
    height: f32,
    app_name: &str,
    status: TopBarStatus,
    flags: TopBarSegmentFlags,
) -> TopBarGeometry {
    let mut segments: Vec<(TopBarSegment, D2D_RECT_F)> = Vec::new();

    // --- Left cluster: mark + app name. ---
    let mark_x = BAR_LEFT_MARK_OFFSET;
    let mark_cx = mark_x + BAR_LEFT_MARK_RADIUS;
    // The mark's pill covers a small tappable area around the circle.
    let mark_right = mark_x + BAR_LEFT_MARK_RADIUS;
    segments.push((
        TopBarSegment::Logo,
        D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: mark_right + 4.0,
            bottom: height,
        },
    ));

    let text_left = mark_right + BAR_LEFT_GAP_AFTER_MARK;
    // Approximate width: ~7.2 DIPs per char at 14pt, clamped so very long
    // names don't eat the whole bar. The actual text is clipped on draw.
    let app_chars = app_name.chars().count().max(1) as f32;
    let app_width = (app_chars * 7.4 + 6.0).min(width * 0.5);
    segments.push((
        TopBarSegment::App,
        D2D_RECT_F {
            left: text_left,
            top: 0.0,
            right: (text_left + app_width).min(width * 0.55),
            bottom: height,
        },
    ));
    let _ = mark_cx;

    // --- Right cluster: stack right-to-left. ---
    let mut cursor_right = width;
    let push_right_icon = |seg: TopBarSegment, cursor: &mut f32| {
        let rect = D2D_RECT_F {
            left: *cursor - BAR_SEGMENT_ICON_WIDTH,
            top: 0.0,
            right: *cursor,
            bottom: height,
        };
        *cursor -= BAR_SEGMENT_ICON_WIDTH + BAR_SEGMENT_GAP;
        (seg, rect)
    };

    // Clock is always rightmost.
    let clock = D2D_RECT_F {
        left: cursor_right - BAR_SEGMENT_CLOCK_WIDTH,
        top: 0.0,
        right: cursor_right,
        bottom: height,
    };
    cursor_right = clock.left - BAR_SEGMENT_GAP;

    let mut right_segments: Vec<(TopBarSegment, D2D_RECT_F)> = Vec::new();
    right_segments.push((TopBarSegment::Clock, clock));

    if flags.show_battery && status.battery_percent.is_some() {
        right_segments.push(push_right_icon(TopBarSegment::Battery, &mut cursor_right));
    }
    if flags.show_volume && status.volume_percent.is_some() {
        right_segments.push(push_right_icon(TopBarSegment::Volume, &mut cursor_right));
    }
    if flags.show_network {
        right_segments.push(push_right_icon(TopBarSegment::Network, &mut cursor_right));
    }
    // Reverse so the slice is left-to-right, then append after the left cluster.
    right_segments.reverse();
    segments.extend(right_segments);

    TopBarGeometry { segments }
}

/// Return the segment whose pill rect contains `x`, if any. The top-bar analog
/// of `dock_hit_test`. `y` is ignored: the whole bar height is hittable.
pub fn top_bar_hit_test(geo: &TopBarGeometry, x: f32) -> Option<TopBarSegment> {
    geo.segments
        .iter()
        .find(|(_, rect)| x >= rect.left && x <= rect.right)
        .map(|(seg, _)| *seg)
}

pub fn dock_hit_test(
    x: f32,
    width: f32,
    count: usize,
    icon_size: f32,
    magnification: f32,
    separator: Option<usize>,
) -> Option<usize> {
    let geometry = dock_geometry(width, count, icon_size, Some(x), magnification, separator);
    geometry
        .icons
        .iter()
        .enumerate()
        .filter(|(_, icon)| x >= icon.left - 3.5 && x <= icon.left + icon.side + 3.5)
        .min_by(|(_, a), (_, b)| (x - a.center).abs().total_cmp(&(x - b.center).abs()))
        .map(|(index, _)| index)
}

fn dock_geometry(
    width: f32,
    count: usize,
    icon_size: f32,
    hover_x: Option<f32>,
    magnification: f32,
    separator: Option<usize>,
) -> DockGeometry {
    let extra_section_gap = separator.map_or(0.0, |_| SECTION_GAP);
    let base_content_width =
        icon_size * count as f32 + ICON_GAP * count.saturating_sub(1) as f32 + extra_section_gap;
    let base_left = (width - base_content_width) / 2.0;
    let step = icon_size + ICON_GAP;
    let sides: Vec<f32> = (0..count)
        .map(|index| {
            let section_offset = separator
                .filter(|separator| index >= *separator)
                .map_or(0.0, |_| SECTION_GAP);
            let base_center = base_left + icon_size / 2.0 + index as f32 * step + section_offset;
            let distance = hover_x
                .map(|cursor| (cursor - base_center).abs() / step)
                .unwrap_or(4.0);
            let influence = smoothstep(1.0 - distance / 2.65);
            icon_size * (1.0 + (magnification - 1.0) * influence)
        })
        .collect();
    let content_width =
        sides.iter().sum::<f32>() + ICON_GAP * count.saturating_sub(1) as f32 + extra_section_gap;
    let mut cursor = (width - content_width) / 2.0;
    let icons = sides
        .into_iter()
        .enumerate()
        .map(|(index, side)| {
            if separator.is_some_and(|separator| index == separator) {
                cursor += SECTION_GAP;
            }
            let left = cursor;
            let center = left + side / 2.0;
            cursor += side + ICON_GAP;
            IconGeometry { center, side, left }
        })
        .collect();
    DockGeometry {
        icons,
        content_width,
    }
}

unsafe fn create_target_bitmap(
    context: &ID2D1DeviceContext,
    swap_chain: &IDXGISwapChain1,
    dpi: f32,
) -> Result<ID2D1Bitmap1> {
    let dxgi_surface: IDXGISurface = unsafe { swap_chain.GetBuffer(0)? };
    let properties = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: dpi,
        dpiY: dpi,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        ..Default::default()
    };
    unsafe {
        context
            .CreateBitmapFromDxgiSurface(&dxgi_surface, Some(&properties))
            .map_err(Into::into)
    }
}

unsafe fn present(surface: &Surface) -> Result<()> {
    unsafe {
        surface.context.EndDraw(None, None)?;
        surface.swap_chain.Present(1, DXGI_PRESENT(0)).ok()?;
    }
    Ok(())
}

fn load_icon_bitmap(
    wic: &IWICImagingFactory,
    target: &ID2D1DeviceContext,
    path: &std::path::Path,
) -> Option<ID2D1Bitmap1> {
    let is_shortcut = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"));
    if is_shortcut {
        if let Some(icon) = shortcut_icon(path) {
            return bitmap_from_icon(wic, target, icon);
        }
        if let Some(resolved) = crate::config::resolve_shortcut(path) {
            if let Some(icon) = resource_icon(&resolved, 0) {
                return bitmap_from_icon(wic, target, icon);
            }
            return shell_image_bitmap(wic, target, &resolved);
        }
        return None;
    }
    if let Some(icon) = resource_icon(path, 0) {
        return bitmap_from_icon(wic, target, icon);
    }
    if let Some(bitmap) = shell_image_bitmap(wic, target, path) {
        return Some(bitmap);
    }
    let icon = file_icon(path)?;
    bitmap_from_icon(wic, target, icon)
}

fn shell_image_bitmap(
    wic: &IWICImagingFactory,
    target: &ID2D1DeviceContext,
    path: &std::path::Path,
) -> Option<ID2D1Bitmap1> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
        let bitmap = factory
            .GetImage(
                SIZE { cx: 256, cy: 256 },
                SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
            )
            .ok()?;
        let source = wic
            .CreateBitmapFromHBITMAP(bitmap, HPALETTE::default(), WICBitmapUsePremultipliedAlpha)
            .ok();
        let _ = DeleteObject(bitmap.into());
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

fn load_stock_icon(
    wic: &IWICImagingFactory,
    target: &ID2D1DeviceContext,
    id: windows::Win32::UI::Shell::SHSTOCKICONID,
) -> Option<ID2D1Bitmap1> {
    let mut info = SHSTOCKICONINFO {
        cbSize: std::mem::size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        SHGetStockIconInfo(id, SHGSI_ICONLOCATION, &mut info).ok()?;
        let mut icon = HICON::default();
        if PrivateExtractIconsW(
            &info.szPath,
            info.iIcon,
            256,
            256,
            Some(std::slice::from_mut(&mut icon)),
            None,
            0,
        ) > 0
            && !icon.is_invalid()
        {
            return bitmap_from_icon(wic, target, icon);
        }
        SHGetStockIconInfo(id, SHGSI_ICON | SHGSI_LARGEICON, &mut info).ok()?;
    }
    (!info.hIcon.is_invalid())
        .then_some(info.hIcon)
        .and_then(|icon| bitmap_from_icon(wic, target, icon))
}

fn file_icon(path: &std::path::Path) -> Option<HICON> {
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
    }
    Some(info.hIcon)
}

fn shortcut_icon(path: &std::path::Path) -> Option<HICON> {
    let shortcut_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        file.Load(PCWSTR(shortcut_wide.as_ptr()), STGM_READ).ok()?;

        let mut icon_path = vec![0u16; 32768];
        let mut icon_index = 0;
        if link
            .GetIconLocation(&mut icon_path, &mut icon_index)
            .is_ok()
        {
            let len = icon_path
                .iter()
                .position(|character| *character == 0)
                .unwrap_or(icon_path.len());
            if len > 0 {
                let configured = String::from_utf16_lossy(&icon_path[..len]);
                let expanded = expand_leading_environment_variable(&configured);
                if let Some(icon) = resource_icon(&expanded, icon_index) {
                    return Some(icon);
                }
                let wide: Vec<u16> = expanded.as_os_str().encode_wide().chain(Some(0)).collect();
                let mut large = HICON::default();
                if ExtractIconExW(PCWSTR(wide.as_ptr()), icon_index, Some(&mut large), None, 1) > 0
                    && !large.is_invalid()
                {
                    return Some(large);
                }
            }
        }
    }
    None
}

fn resource_icon(path: &std::path::Path, index: i32) -> Option<HICON> {
    if !path.is_file() {
        return None;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if wide.len() > 260 {
        return None;
    }
    let mut resource = [0u16; 260];
    resource[..wide.len()].copy_from_slice(&wide);
    let mut icon = HICON::default();
    unsafe {
        (PrivateExtractIconsW(
            &resource,
            index,
            256,
            256,
            Some(std::slice::from_mut(&mut icon)),
            None,
            0,
        ) > 0
            && !icon.is_invalid())
        .then_some(icon)
    }
}

fn expand_leading_environment_variable(value: &str) -> PathBuf {
    let Some(rest) = value.strip_prefix('%') else {
        return PathBuf::from(value);
    };
    let Some(end) = rest.find('%') else {
        return PathBuf::from(value);
    };
    let variable = &rest[..end];
    let suffix = &rest[end + 1..];
    std::env::var_os(variable)
        .map(PathBuf::from)
        .map(|root| root.join(suffix.trim_start_matches(['\\', '/'])))
        .unwrap_or_else(|| PathBuf::from(value))
}

fn bitmap_from_icon(
    wic: &IWICImagingFactory,
    target: &ID2D1DeviceContext,
    icon: HICON,
) -> Option<ID2D1Bitmap1> {
    unsafe {
        let source = wic.CreateBitmapFromHICON(icon).ok();
        let _ = DestroyIcon(icon);
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

fn smoothstep(value: f32) -> f32 {
    let x = value.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn color(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

/// Convenience accessors for Direct2D rects used throughout the menu bar.
trait RectExt {
    fn width(&self) -> f32;
    #[allow(dead_code)]
    fn height(&self) -> f32;
    fn center_x(&self) -> f32;
    fn center_y(&self) -> f32;
}

impl RectExt for D2D_RECT_F {
    fn width(&self) -> f32 {
        (self.right - self.left).max(0.0)
    }
    fn height(&self) -> f32 {
        (self.bottom - self.top).max(0.0)
    }
    fn center_x(&self) -> f32 {
        (self.left + self.right) / 2.0
    }
    fn center_y(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }
}

/// Draw the rounded hover "pill" behind a segment, inset vertically.
unsafe fn fill_pill(
    context: &ID2D1DeviceContext,
    rect: &D2D_RECT_F,
    brush: &ID2D1SolidColorBrush,
) {
    unsafe {
        context.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: rect.left + 2.0,
                    top: BAR_PILL_INSET_Y,
                    right: rect.right - 2.0,
                    bottom: rect.bottom - BAR_PILL_INSET_Y,
                },
                radiusX: 6.0,
                radiusY: 6.0,
            },
            brush,
        );
    }
}

/// Draw a path geometry centered inside `rect`, scaled to fit a `w`x`h` box.
/// The icon's local-space points are mapped via a 3x2 affine transform set on
/// the render target for the duration of the draw.
unsafe fn draw_icon(
    context: &ID2D1DeviceContext,
    geometry: &ID2D1PathGeometry,
    rect: &D2D_RECT_F,
    w: f32,
    h: f32,
    brush: &ID2D1SolidColorBrush,
) {
    unsafe {
        // Author coordinates are small (<= ~24 units); scale to target size
        // and center within the segment rect.
        let scale_x = w / 18.0;
        let scale_y = h / 18.0;
        let scale = scale_x.min(scale_y);
        let dx = rect.center_x() - 9.0 * scale;
        let dy = rect.center_y() - 8.0 * scale;
        let mut original = windows_numerics::Matrix3x2::default();
        context.GetTransform(&mut original);
        let m = windows_numerics::Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: scale,
            M31: dx,
            M32: dy,
        };
        context.SetTransform(&m);
        let _ = context.FillGeometry(geometry, brush, None::<&ID2D1Brush>);
        context.SetTransform(&original);
    }
}

/// A closed outline authored in local coordinates: a start point and a list of
/// line-to points. Kept small so each icon's silhouette reads at a glance.
struct LocalFigure {
    start: (f32, f32),
    lines: Vec<(f32, f32)>,
}

fn build_geometry(
    factory: &ID2D1Factory,
    figure: fn() -> LocalFigure,
) -> Option<ID2D1PathGeometry> {
    let shape = figure();
    unsafe {
        let geometry = factory.CreatePathGeometry().ok()?;
        let sink = geometry.Open().ok()?;
        sink.SetFillMode(D2D1_FILL_MODE_WINDING);
        sink.BeginFigure(vec2(shape.start.0, shape.start.1), D2D1_FIGURE_BEGIN_FILLED);
        if !shape.lines.is_empty() {
            let points: Vec<_> = shape.lines.iter().map(|(x, y)| vec2(*x, *y)).collect();
            sink.AddLines(&points);
        }
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close().ok()?;
        Some(geometry)
    }
}

fn vec2(x: f32, y: f32) -> windows_numerics::Vector2 {
    windows_numerics::Vector2 { X: x, Y: y }
}

/// Wi-Fi glyph: three nested arcs approximated as a filled wedge silhouette,
/// centered on (9, 16), spanning ~18×16 local units.
fn wifi_figure() -> LocalFigure {
    LocalFigure {
        start: (9.0, 16.0),
        lines: vec![
            (2.5, 9.5),
            (4.5, 9.5),
            (9.0, 13.0),
            (13.5, 9.5),
            (15.5, 9.5),
            (11.5, 14.5),
            (9.0, 16.0),
        ],
    }
}

/// Speaker glyph: trapezoid cone + body, ~16×16 local units.
fn speaker_figure() -> LocalFigure {
    LocalFigure {
        start: (2.0, 6.0),
        lines: vec![
            (6.0, 6.0),
            (10.0, 2.0),
            (10.0, 14.0),
            (6.0, 10.0),
            (2.0, 10.0),
            (2.0, 6.0),
        ],
    }
}

/// Muted speaker: same body + a diagonal slash, drawn as a single outline so a
/// stroke of this geometry crosses the speaker — the shape difference conveys
/// "muted" without relying on color alone.
fn speaker_muted_figure() -> LocalFigure {
    LocalFigure {
        start: (2.0, 6.0),
        lines: vec![
            (6.0, 6.0),
            (10.0, 2.0),
            (10.0, 14.0),
            (6.0, 10.0),
            (2.0, 10.0),
            (2.0, 6.0),
            (4.0, 6.0),
            (12.5, 14.5),
            (13.5, 13.5),
            (5.0, 5.0),
            (2.0, 6.0),
        ],
    }
}

/// Battery outline (a rounded-rect-ish capsule) in ~24×12 local units. The fill
/// level is rendered separately as a clipped rect, not part of this outline.
fn battery_outline_figure() -> LocalFigure {
    LocalFigure {
        start: (1.5, 3.5),
        lines: vec![
            (1.5, 8.5),
            (19.5, 8.5),
            (19.5, 3.5),
            (1.5, 3.5),
        ],
    }
}

/// Lightning bolt for the charging indicator, ~10×16 local units.
fn bolt_figure() -> LocalFigure {
    LocalFigure {
        start: (6.0, 1.0),
        lines: vec![
            (1.5, 9.0),
            (4.5, 9.0),
            (3.0, 15.0),
            (8.5, 6.5),
            (5.5, 6.5),
            (7.0, 1.0),
            (6.0, 1.0),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::{
        dock_geometry, smoothstep, top_bar_geometry, top_bar_hit_test, RectExt, TopBarSegment,
        TopBarSegmentFlags, TopBarStatus,
    };

    fn full_status() -> TopBarStatus {
        TopBarStatus {
            battery_percent: Some(80),
            charging: false,
            network_online: true,
            volume_percent: Some(50),
            muted: false,
        }
    }

    fn all_on() -> TopBarSegmentFlags {
        TopBarSegmentFlags {
            show_network: true,
            show_volume: true,
            show_battery: true,
        }
    }

    #[test]
    fn magnification_falloff_is_bounded() {
        assert_eq!(smoothstep(-1.0), 0.0);
        assert_eq!(smoothstep(2.0), 1.0);
        assert!((smoothstep(0.5) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn magnification_tracks_fractional_cursor_motion() {
        let first = dock_geometry(800.0, 5, 48.0, Some(290.0), 1.42, None);
        let second = dock_geometry(800.0, 5, 48.0, Some(296.0), 1.42, None);
        assert_ne!(first.icons[1].side, second.icons[1].side);
    }

    #[test]
    fn utility_separator_adds_mac_style_section_space() {
        let plain = dock_geometry(800.0, 5, 48.0, None, 1.0, None);
        let sectioned = dock_geometry(800.0, 5, 48.0, None, 1.0, Some(4));
        assert!((sectioned.content_width - plain.content_width - 13.0).abs() < f32::EPSILON);
    }

    #[test]
    fn clock_segment_is_flush_with_right_edge() {
        let geo = top_bar_geometry(1920.0, 32.0, "Edge", full_status(), all_on());
        let clock = geo.find(TopBarSegment::Clock).expect("clock segment exists");
        assert!(
            (clock.right - 1920.0).abs() < f32::EPSILON,
            "clock right edge {} should be flush with bar width",
            clock.right
        );
    }

    #[test]
    fn disabled_flags_drop_their_segments() {
        let flags = TopBarSegmentFlags {
            show_network: false,
            show_volume: false,
            show_battery: false,
        };
        let geo = top_bar_geometry(1920.0, 32.0, "Edge", full_status(), flags);
        assert!(geo.find(TopBarSegment::Network).is_none());
        assert!(geo.find(TopBarSegment::Volume).is_none());
        assert!(geo.find(TopBarSegment::Battery).is_none());
        // Logo, App, and Clock remain.
        assert!(geo.find(TopBarSegment::Logo).is_some());
        assert!(geo.find(TopBarSegment::App).is_some());
        assert!(geo.find(TopBarSegment::Clock).is_some());
    }

    #[test]
    fn missing_battery_drops_segment_even_when_flag_on() {
        let status = TopBarStatus {
            battery_percent: None,
            ..full_status()
        };
        let geo = top_bar_geometry(1920.0, 32.0, "Edge", status, all_on());
        assert!(geo.find(TopBarSegment::Battery).is_none());
    }

    #[test]
    fn hit_test_finds_clock_and_misses_gap() {
        let geo = top_bar_geometry(1920.0, 32.0, "Edge", full_status(), all_on());
        let clock = geo.find(TopBarSegment::Clock).unwrap();
        // Inside the clock segment → Clock.
        assert_eq!(
            top_bar_hit_test(&geo, clock.center_x()),
            Some(TopBarSegment::Clock)
        );
        // Well to the left of every right-side segment → either Logo/App or None,
        // but never a status segment.
        let hit = top_bar_hit_test(&geo, 400.0);
        assert!(matches!(
            hit,
            Some(TopBarSegment::App) | Some(TopBarSegment::Logo) | None
        ));
    }

    #[test]
    fn hit_test_distinguishes_adjacent_segments() {
        let geo = top_bar_geometry(1920.0, 32.0, "Edge", full_status(), all_on());
        let volume = geo.find(TopBarSegment::Volume).unwrap();
        let battery = geo.find(TopBarSegment::Battery).unwrap();
        assert_eq!(
            top_bar_hit_test(&geo, volume.center_x()),
            Some(TopBarSegment::Volume)
        );
        assert_eq!(
            top_bar_hit_test(&geo, battery.center_x()),
            Some(TopBarSegment::Battery)
        );
    }
}
