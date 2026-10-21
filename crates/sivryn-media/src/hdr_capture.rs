//! Screen capture from HDR displays, tone-mapped to SDR.
//!
//! libwebrtc's capturer reads 8-bit frames, which from an HDR desktop look
//! washed out. For sources on an HDR display we capture with Windows
//! Graphics Capture in 16-bit float (scRGB: linear, BT.709 primaries,
//! 1.0 = 80 nits) and convert on the GPU with a compute shader:
//!
//! 1. divide by the SDR white level Windows composes SDR content at (the
//!    "SDR content brightness" setting), so the desktop and SDR apps come out
//!    exactly as they look on an SDR screen;
//! 2. keep everything up to a knee as it is and compress the highlights
//!    above it smoothly into the remaining range (no hard clipping);
//! 3. encode as sRGB.
//!
//! The result (RGBA, 8 bit) goes into the normal sending path.

#![cfg(windows)]

use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS,
    QueryDisplayConfig,
};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1, IDXGIOutput6};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICE_ACTIVE, DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITOR_DEFAULTTONEAREST, MONITORINFOEXW, MonitorFromWindow,
};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::{BOOL, Interface, PCSTR, s};

use crate::screen::{ScreenSource, SourceKind};

/// Brightness information of an HDR display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HdrDisplay {
    monitor: isize,
    /// SDR white in scRGB units (1.0 = 80 nits).
    pub sdr_white: f32,
    /// Peak brightness in scRGB units.
    pub peak: f32,
}

/// The display a source is on, if HDR is turned on for it.
pub fn hdr_display(source: &ScreenSource) -> Option<HdrDisplay> {
    let monitor = monitor_of(source)?;
    // SAFETY: plain DXGI enumeration; interfaces are released on drop.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            a += 1;
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                o += 1;
                let Ok(output6) = output.cast::<IDXGIOutput6>() else { continue };
                let Ok(desc) = output6.GetDesc1() else { continue };
                if desc.Monitor != monitor {
                    continue;
                }
                if desc.ColorSpace != DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020 {
                    return None;
                }
                let sdr_white = sdr_white_level(monitor).unwrap_or(2.5);
                let peak = (desc.MaxLuminance / 80.0).max(sdr_white);
                return Some(HdrDisplay { monitor: monitor.0 as isize, sdr_white, peak });
            }
        }
    }
    None
}

/// The monitor a screen (libwebrtc: display device index) or window (HWND) is on.
fn monitor_of(source: &ScreenSource) -> Option<HMONITOR> {
    match source.kind {
        SourceKind::Window => {
            // SAFETY: MonitorFromWindow accepts any window handle.
            let m = unsafe { MonitorFromWindow(HWND(source.id as *mut _), MONITOR_DEFAULTTONEAREST) };
            (!m.is_invalid()).then_some(m)
        }
        SourceKind::Screen => {
            let name = display_device_name(source.id as u32)?;
            find_monitor(|info| wide_eq(&info.szDevice, &name))
        }
    }
}

fn display_device_name(index: u32) -> Option<Vec<u16>> {
    let mut device = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
    // SAFETY: the struct is sized as the API expects.
    unsafe { EnumDisplayDevicesW(None, index, &mut device, 0) }.as_bool().then_some(())?;
    if !device.StateFlags.contains(DISPLAY_DEVICE_ACTIVE) {
        return None;
    }
    Some(device.DeviceName.to_vec())
}

fn wide_eq(a: &[u16], b: &[u16]) -> bool {
    let trim = |s: &[u16]| s.iter().take_while(|c| **c != 0).copied().collect::<Vec<u16>>();
    trim(a) == trim(b)
}

/// The first monitor whose info matches.
fn find_monitor(matches: impl Fn(&MONITORINFOEXW) -> bool) -> Option<HMONITOR> {
    let mut monitors: Vec<HMONITOR> = Vec::new();
    unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the Vec passed below, alive during the call.
        unsafe { (*(data.0 as *mut Vec<HMONITOR>)).push(m) };
        BOOL(1)
    }
    // SAFETY: the callback only pushes into the Vec.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut monitors as *mut _ as isize));
    }
    monitors.into_iter().find(|m| {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: MONITORINFOEXW starts with MONITORINFO, sized accordingly.
        unsafe { GetMonitorInfoW(*m, &mut info as *mut _ as *mut _) }.as_bool() && matches(&info)
    })
}

/// Windows' SDR content brightness for a monitor, in scRGB units.
fn sdr_white_level(monitor: HMONITOR) -> Option<f32> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: as in find_monitor; then the DisplayConfig queries with sized buffers.
    unsafe {
        GetMonitorInfoW(monitor, &mut info as *mut _ as *mut _).as_bool().then_some(())?;
        let (mut paths, mut modes) = (0u32, 0u32);
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes).ok().ok()?;
        let mut path_info = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
        let mut mode_info = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut paths,
            path_info.as_mut_ptr(),
            &mut modes,
            mode_info.as_mut_ptr(),
            None,
        )
        .ok()
        .ok()?;
        for path in path_info.iter().take(paths as usize) {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: path.sourceInfo.adapterId,
                    id: path.sourceInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0
                || !wide_eq(&source.viewGdiDeviceName, &info.szDevice)
            {
                continue;
            }
            let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                    size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                    adapterId: path.targetInfo.adapterId,
                    id: path.targetInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut white.header) == 0 {
                // Thousandths of 80 nits, i.e. scRGB × 1000.
                return Some((white.SDRWhiteLevel as f32 / 1000.0).max(1.0));
            }
        }
    }
    None
}

const SHADER: &str = r#"
Texture2D<float4> src : register(t0);
RWTexture2D<unorm float4> dst : register(u0);
cbuffer Params : register(b0) { float sdr_white; float knee; float2 pad; };

float3 to_srgb(float3 c) {
    return c <= 0.0031308 ? c * 12.92 : 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

[numthreads(16, 16, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint w, h;
    dst.GetDimensions(w, h);
    if (id.x >= w || id.y >= h) return;
    // scRGB to SDR-relative linear light: 1.0 is SDR white.
    float3 c = max(src[id.xy].rgb, 0.0) / sdr_white;
    // Up to the knee unchanged; above it the highlights roll off smoothly
    // towards 1.0 instead of clipping (applied to the brightest channel so
    // the hue stays).
    float m = max(c.r, max(c.g, c.b));
    if (m > knee) {
        float rolled = knee + (1.0 - knee) * (1.0 - exp(-(m - knee) / (1.0 - knee)));
        c *= rolled / m;
    }
    dst[id.xy] = float4(to_srgb(saturate(c)), 1.0);
}
"#;

/// Up to this fraction of SDR white nothing changes.
const KNEE: f32 = 0.85;

/// A running HDR capture; frames come from [`HdrCapture::next_frame`] on the
/// thread that created it.
pub struct HdrCapture {
    context: ID3D11DeviceContext,
    device: ID3D11Device,
    shader: ID3D11ComputeShader,
    params: ID3D11Buffer,
    pool: Direct3D11CaptureFramePool,
    _session: windows::Graphics::Capture::GraphicsCaptureSession,
    winrt_device: IDirect3DDevice,
    target: Option<(u32, u32, ID3D11Texture2D, ID3D11UnorderedAccessView, ID3D11Texture2D)>,
}

impl HdrCapture {
    pub fn start(source: &ScreenSource, display: HdrDisplay) -> windows::core::Result<Self> {
        // SAFETY: COM/WinRT and D3D11 setup on this thread; every interface
        // is reference counted by the `windows` crate.
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.expect("device created");
            let context = context.expect("context created");
            let dxgi: IDXGIDevice = device.cast()?;
            let winrt_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;

            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = match source.kind {
                SourceKind::Screen => interop.CreateForMonitor(HMONITOR(display.monitor as *mut _))?,
                SourceKind::Window => interop.CreateForWindow(HWND(source.id as *mut _))?,
            };
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &winrt_device,
                DirectXPixelFormat::R16G16B16A16Float,
                2,
                item.Size()?,
            )?;
            let session = pool.CreateCaptureSession(&item)?;
            let _ = session.SetIsCursorCaptureEnabled(true);
            // Windows 11: no yellow frame around what is captured.
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture()?;

            let shader = compile_shader(&device)?;
            let params_data: [f32; 4] = [display.sdr_white, KNEE, 0.0, 0.0];
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: 16,
                Usage: D3D11_USAGE_IMMUTABLE,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            let init = D3D11_SUBRESOURCE_DATA { pSysMem: params_data.as_ptr() as *const _, ..Default::default() };
            let mut params = None;
            device.CreateBuffer(&desc, Some(&init), Some(&mut params))?;

            Ok(Self {
                context,
                device,
                shader,
                params: params.expect("buffer created"),
                pool,
                _session: session,
                winrt_device,
                target: None,
            })
        }
    }

    /// The newest captured frame as SDR RGBA (`width × height × 4`), or
    /// `None` when nothing new arrived.
    pub fn next_frame(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        let frame = self.pool.TryGetNextFrame().ok()?;
        let size = frame.ContentSize().ok()?;
        // SAFETY: D3D11 calls on the device this capture owns; the mapped
        // staging memory is copied out before unmapping.
        unsafe {
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface().ok()?.cast().ok()?;
            let texture: ID3D11Texture2D = access.GetInterface().ok()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let (w, h) = ((size.Width as u32).min(desc.Width), (size.Height as u32).min(desc.Height));
            if w == 0 || h == 0 {
                return None;
            }
            // Windows and screens change size: rebuild the pool and targets.
            if desc.Width != size.Width as u32 || desc.Height != size.Height as u32 {
                let _ = self.pool.Recreate(&self.winrt_device, DirectXPixelFormat::R16G16B16A16Float, 2, size);
            }
            if self.target.as_ref().is_none_or(|t| t.0 != desc.Width || t.1 != desc.Height) {
                self.target = Some(self.create_targets(desc.Width, desc.Height).ok()?);
            }
            let (_, _, out, uav, staging) = self.target.as_ref()?;

            let mut srv = None;
            self.device.CreateShaderResourceView(&texture, None, Some(&mut srv)).ok()?;
            self.context.CSSetShader(&self.shader, None);
            self.context.CSSetShaderResources(0, Some(&[srv]));
            self.context.CSSetUnorderedAccessViews(0, 1, Some(&Some(uav.clone())), None);
            self.context.CSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            self.context.Dispatch(desc.Width.div_ceil(16), desc.Height.div_ceil(16), 1);
            self.context.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
            self.context.CSSetShaderResources(0, Some(&[None]));
            self.context.CopyResource(staging, out);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).ok()?;
            let mut data = vec![0u8; (w * h * 4) as usize];
            for y in 0..h as usize {
                let src = std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    w as usize * 4,
                );
                data[y * w as usize * 4..(y + 1) * w as usize * 4].copy_from_slice(src);
            }
            self.context.Unmap(staging, 0);
            Some((data, w, h))
        }
    }

    #[allow(clippy::type_complexity)]
    fn create_targets(
        &self,
        w: u32,
        h: u32,
    ) -> windows::core::Result<(u32, u32, ID3D11Texture2D, ID3D11UnorderedAccessView, ID3D11Texture2D)> {
        let mut desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
            ..Default::default()
        };
        // SAFETY: texture and view creation with filled descriptors.
        unsafe {
            let mut out = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut out))?;
            let out = out.expect("texture created");
            let mut uav = None;
            self.device.CreateUnorderedAccessView(&out, None, Some(&mut uav))?;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            Ok((w, h, out, uav.expect("view created"), staging.expect("texture created")))
        }
    }
}

fn compile_shader(device: &ID3D11Device) -> windows::core::Result<ID3D11ComputeShader> {
    // SAFETY: D3DCompile with a static source; the blob outlives its use.
    unsafe {
        let mut code: Option<ID3DBlob> = None;
        let mut errors: Option<ID3DBlob> = None;
        let result = D3DCompile(
            SHADER.as_ptr() as *const _,
            SHADER.len(),
            PCSTR::null(),
            None,
            None,
            s!("main"),
            s!("cs_5_0"),
            0,
            0,
            &mut code,
            Some(&mut errors),
        );
        if let Err(e) = result {
            if let Some(errors) = errors {
                let msg = std::slice::from_raw_parts(errors.GetBufferPointer() as *const u8, errors.GetBufferSize());
                tracing::error!(error = %String::from_utf8_lossy(msg), "HDR tone mapping shader failed to compile");
            }
            return Err(e);
        }
        let code = code.expect("shader compiled");
        let bytes = std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize());
        let mut shader = None;
        device.CreateComputeShader(bytes, None, Some(&mut shader))?;
        Ok(shader.expect("shader created"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captures one frame of the first screen through the tone-mapping path
    /// (works on SDR screens too: their scRGB values just stay below 1.0).
    /// Needs a desktop session, so it only runs with SIVRYN_TEST_CAPTURE set.
    #[test]
    fn tone_mapped_capture_produces_frames() {
        if std::env::var_os("SIVRYN_TEST_CAPTURE").is_none() {
            return;
        }
        let sources = crate::screen::list_sources();
        let screen = sources.iter().find(|s| s.kind == SourceKind::Screen).expect("a screen");
        let monitor = monitor_of(screen).expect("its monitor");
        eprintln!("HDR: {:?}", hdr_display(screen));
        let display =
            hdr_display(screen).unwrap_or(HdrDisplay { monitor: monitor.0 as isize, sdr_white: 1.0, peak: 1.0 });
        let mut capture = HdrCapture::start(screen, display).expect("capture starts");
        let mut frame = None;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(40));
            if let Some(f) = capture.next_frame() {
                frame = Some(f);
                break;
            }
        }
        let (data, w, h) = frame.expect("a frame");
        assert_eq!(data.len(), (w * h * 4) as usize);
        // Not all black: the desktop has something on it.
        assert!(data.chunks(4).any(|p| p[0] > 20 || p[1] > 20 || p[2] > 20));
        eprintln!("frame {w}x{h}");
        // A BMP of the frame for looking at, if asked for.
        if let Some(path) = std::env::var_os("SIVRYN_TEST_CAPTURE_BMP") {
            let mut bmp = Vec::new();
            let size = 54 + data.len() as u32;
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&size.to_le_bytes());
            bmp.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
            bmp.extend_from_slice(&(w as i32).to_le_bytes());
            bmp.extend_from_slice(&(-(h as i32)).to_le_bytes());
            bmp.extend_from_slice(&[1, 0, 32, 0, 0, 0, 0, 0]);
            bmp.extend_from_slice(&(data.len() as u32).to_le_bytes());
            bmp.extend_from_slice(&[0; 16]);
            for p in data.chunks(4) {
                bmp.extend_from_slice(&[p[2], p[1], p[0], 255]);
            }
            std::fs::write(path, bmp).unwrap();
        }
    }
}
