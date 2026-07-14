use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemStatus {
    pub battery_percent: Option<u8>,
    pub charging: bool,
    pub network_online: bool,
    pub volume_percent: Option<u8>,
    pub muted: bool,
}

pub fn read_status() -> SystemStatus {
    let mut power = SYSTEM_POWER_STATUS::default();
    let ok = unsafe { GetSystemPowerStatus(&mut power).is_ok() };
    let network_online = unsafe {
        let mut flags = windows::Win32::Networking::WinInet::INTERNET_CONNECTION::default();
        windows::Win32::Networking::WinInet::InternetGetConnectedState(&mut flags, None).is_ok()
    };
    let (volume_percent, muted) = read_audio().unwrap_or((None, false));
    SystemStatus {
        battery_percent: (ok && power.BatteryLifePercent <= 100)
            .then_some(power.BatteryLifePercent),
        charging: ok && power.ACLineStatus == 1,
        network_online,
        volume_percent,
        muted,
    }
}

fn read_audio() -> Option<(Option<u8>, bool)> {
    use windows::Win32::{
        Media::Audio::{
            Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia,
            eRender,
        },
        System::Com::{CLSCTX_ALL, CoCreateInstance},
    };
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .ok()?;
        let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        let volume = endpoint
            .GetMasterVolumeLevelScalar()
            .ok()
            .map(|v| (v.clamp(0.0, 1.0) * 100.0).round() as u8);
        let muted = endpoint.GetMute().ok().is_some_and(|m| m.as_bool());
        Some((volume, muted))
    }
}
