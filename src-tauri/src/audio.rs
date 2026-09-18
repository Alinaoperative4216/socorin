//! The microphone for recordings: which inputs exist, which one a recording
//! takes (Settings → Recording, or the system default), and the platform
//! pieces that list and open them.
//!
//! macOS asks AVFoundation for the devices (`macos::audio_inputs`); their
//! `uniqueID` is what `screencapture -G` takes. Windows asks WASAPI
//! (`wasapi::inputs`), and the built-in recorder reads the microphone
//! through WASAPI too (`wasapi::Capture`, paced into the Media Foundation
//! encoder by `wgc`). Linux asks PulseAudio / PipeWire through `pactl`
//! (`pulse`); ffmpeg then records the source by name (`-f pulse`).
//!
//! `choose` (settings + devices → what to record) and the `pactl` parser are
//! platform-independent and unit-tested everywhere.

use serde::{Deserialize, Serialize};

use crate::settings::Settings;

/// One microphone, as the settings window lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    /// What the platform calls it: the AVFoundation unique id, the WASAPI
    /// endpoint id or the PulseAudio source name. Stored in the settings.
    pub id: String,
    pub name: String,
    /// The system's default input right now.
    pub default: bool,
}

/// What a recording takes as sound.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Choice {
    /// The microphone, or `None` for a silent recording.
    pub input: Option<Input>,
    /// `input` is the system default: the recorder asks for "the default
    /// input" rather than for that device by id, so a default that changes
    /// as the recording starts is followed.
    pub default: bool,
    /// Why there is no sound, or why another microphone than the chosen
    /// one is recorded. Shown on the recording bar.
    pub issue: Option<String>,
}

impl Choice {
    /// Drops the sound because of `why` (the recorder could not open the
    /// microphone, the permission is missing, …). Linux has neither case:
    /// ffmpeg opens the source itself.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub fn mute(&mut self, why: String) {
        self.input = None;
        self.default = false;
        self.issue = Some(why);
    }
}

/// Said when the microphone is wanted but none is there.
pub const NO_MIC: &str = "No microphone is connected; recording without sound.";

/// The settings' choice among the microphones present: the system default
/// (or the first one where no default is flagged) unless a device was
/// chosen; a chosen device that is not connected is stood in for by the
/// default, and the bar says so.
pub fn choose(settings: &Settings, inputs: &[Input]) -> Choice {
    if !settings.mic {
        return Choice::default();
    }
    let Some(default) = inputs.iter().find(|i| i.default).or_else(|| inputs.first()) else {
        return Choice { input: None, default: false, issue: Some(NO_MIC.into()) };
    };
    if settings.mic_device.is_empty() {
        return Choice { input: Some(default.clone()), default: true, issue: None };
    }
    match inputs.iter().find(|i| i.id == settings.mic_device) {
        Some(input) => Choice { input: Some(input.clone()), default: false, issue: None },
        None => {
            let name = if settings.mic_device_name.is_empty() {
                "The chosen microphone"
            } else {
                settings.mic_device_name.as_str()
            };
            Choice {
                input: Some(default.clone()),
                default: true,
                issue: Some(format!("{name} is not connected; recording with {}.", default.name)),
            }
        }
    }
}

/// The microphones present, in the platform's order. Empty where there is
/// none, or where they cannot be listed (a recording is then silent, and
/// the bar says so).
pub fn inputs() -> Vec<Input> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::audio_inputs()
    }
    #[cfg(windows)]
    {
        wasapi::inputs().unwrap_or_else(|e| {
            eprintln!("[audio] {e}");
            Vec::new()
        })
    }
    #[cfg(target_os = "linux")]
    {
        pulse::inputs()
    }
}

/// Linux: PulseAudio / PipeWire sources through `pactl`. Compiled (and
/// tested) everywhere for its parsers; only Linux calls `inputs`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod pulse {
    use std::process::{Command, Stdio};

    use super::Input;

    pub fn inputs() -> Vec<Input> {
        let list = pactl(&["list", "sources"]);
        // `get-default-source` needs pactl 15 (2021); `info` has said it for ever.
        let mut default = pactl(&["get-default-source"]).trim().to_string();
        if default.is_empty() {
            default = default_from_info(&pactl(&["info"]));
        }
        parse_sources(&list, &default)
    }

    fn pactl(args: &[&str]) -> String {
        Command::new("pactl")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }

    /// The "Default Source: …" line of `pactl info`.
    pub(crate) fn default_from_info(info: &str) -> String {
        info.lines()
            .filter_map(|l| l.trim().strip_prefix("Default Source:"))
            .map(|v| v.trim().to_string())
            .next()
            .unwrap_or_default()
    }

    /// `pactl list sources`: one block per source ("Source #N"), with
    /// "Name:" and "Description:" lines. Monitors (what the computer plays,
    /// `….monitor`) are not microphones and are left out.
    pub(crate) fn parse_sources(text: &str, default: &str) -> Vec<Input> {
        let mut inputs = Vec::new();
        let mut name = String::new();
        let mut description = String::new();
        let flush = |name: &mut String, description: &mut String, inputs: &mut Vec<Input>| {
            if !name.is_empty() && !name.ends_with(".monitor") {
                inputs.push(Input {
                    id: name.clone(),
                    name: if description.is_empty() { name.clone() } else { description.clone() },
                    default: name == default,
                });
            }
            name.clear();
            description.clear();
        };
        for line in text.lines() {
            let trimmed = line.trim();
            if line.starts_with("Source #") {
                flush(&mut name, &mut description, &mut inputs);
            } else if let Some(v) = trimmed.strip_prefix("Name:") {
                name = v.trim().to_string();
            } else if let Some(v) = trimmed.strip_prefix("Description:") {
                description = v.trim().to_string();
            }
        }
        flush(&mut name, &mut description, &mut inputs);
        inputs
    }
}

/// Windows: the microphones and their audio through WASAPI. Whatever a
/// device's own format is, the stream comes out as 16-bit stereo PCM at
/// 48 kHz (`AUTOCONVERTPCM`), which is what the encoder is set up for.
#[cfg(windows)]
pub(crate) mod wasapi {
    use windows::{
        core::{PCWSTR, PWSTR},
        Win32::{
            Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
            Media::Audio::{
                eCapture, eConsole, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
                MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE,
                WAVEFORMATEX, WAVE_FORMAT_PCM,
            },
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, StructuredStorage::PropVariantToStringAlloc,
                CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
            },
        },
    };

    use super::Input;
    use crate::wgc::{AudioSource, PCM_BLOCK, PCM_CHANNELS, PCM_RATE};

    /// COM on this thread (multi-threaded apartment; a thread that already
    /// has one, of either kind, is fine as well).
    fn com_init() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
    }

    fn enumerator() -> Result<IMMDeviceEnumerator, String> {
        com_init();
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| format!("cannot list audio devices: {e}"))
    }

    fn id_of(device: &IMMDevice) -> Result<String, String> {
        unsafe {
            let id: PWSTR = device.GetId().map_err(|e| format!("cannot read an audio device id: {e}"))?;
            let text = id.to_string().map_err(|e| format!("audio device id is not UTF-16: {e}"));
            CoTaskMemFree(Some(id.0 as *const _));
            text
        }
    }

    /// The name the Sound settings show; empty when it cannot be read.
    fn name_of(device: &IMMDevice) -> String {
        unsafe {
            let Ok(store) = device.OpenPropertyStore(STGM_READ) else {
                return String::new();
            };
            let Ok(value) = store.GetValue(&PKEY_Device_FriendlyName) else {
                return String::new();
            };
            match PropVariantToStringAlloc(&value) {
                Ok(text) => {
                    let name = text.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(text.0 as *const _));
                    name
                }
                Err(_) => String::new(),
            }
        }
    }

    /// The active capture endpoints, the console default flagged.
    pub fn inputs() -> Result<Vec<Input>, String> {
        let enumerator = enumerator()?;
        unsafe {
            let default_id = enumerator
                .GetDefaultAudioEndpoint(eCapture, eConsole)
                .ok()
                .and_then(|d| id_of(&d).ok());
            let devices = enumerator
                .EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)
                .map_err(|e| format!("cannot list microphones: {e}"))?;
            let count = devices.GetCount().map_err(|e| format!("cannot list microphones: {e}"))?;
            let mut list = Vec::new();
            for n in 0..count {
                let Ok(device) = devices.Item(n) else { continue };
                let Ok(id) = id_of(&device) else { continue };
                let name = name_of(&device);
                list.push(Input {
                    default: default_id.as_deref() == Some(id.as_str()),
                    name: if name.is_empty() { id.clone() } else { name },
                    id,
                });
            }
            Ok(list)
        }
    }

    /// A microphone being read.
    pub struct Capture {
        client: IAudioClient,
        reader: IAudioCaptureClient,
    }

    impl Capture {
        /// Opens `device_id` (`None`: the default input) in shared mode
        /// and starts the stream. The Windows privacy switch for the
        /// microphone makes this fail, as does a device that is gone.
        pub fn open(device_id: Option<&str>) -> Result<Self, String> {
            let enumerator = enumerator()?;
            unsafe {
                let device = match device_id {
                    Some(id) => {
                        let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
                        enumerator.GetDevice(PCWSTR(wide.as_ptr()))
                    }
                    None => enumerator.GetDefaultAudioEndpoint(eCapture, eConsole),
                }
                .map_err(|e| format!("no microphone: {e}"))?;
                let client: IAudioClient = device
                    .Activate(CLSCTX_ALL, None)
                    .map_err(|e| format!("cannot open the microphone: {e}"))?;
                let block = PCM_BLOCK as u32;
                let format = WAVEFORMATEX {
                    wFormatTag: WAVE_FORMAT_PCM as u16,
                    nChannels: PCM_CHANNELS as u16,
                    nSamplesPerSec: PCM_RATE,
                    nAvgBytesPerSec: PCM_RATE * block,
                    nBlockAlign: block as u16,
                    wBitsPerSample: (block / PCM_CHANNELS * 8) as u16,
                    cbSize: 0,
                };
                // A 200 ms buffer: the pacer empties it every 1/30 s.
                client
                    .Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                        2_000_000,
                        0,
                        &format,
                        None,
                    )
                    .map_err(|e| format!("cannot open the microphone: {e}"))?;
                let reader: IAudioCaptureClient = client
                    .GetService()
                    .map_err(|e| format!("cannot read the microphone: {e}"))?;
                client.Start().map_err(|e| format!("cannot start the microphone: {e}"))?;
                Ok(Self { client, reader })
            }
        }
    }

    impl AudioSource for Capture {
        fn pull(&mut self) -> Result<Vec<u8>, String> {
            let mut out = Vec::new();
            unsafe {
                loop {
                    let frames = self
                        .reader
                        .GetNextPacketSize()
                        .map_err(|e| format!("the microphone stopped: {e}"))?;
                    if frames == 0 {
                        break;
                    }
                    let mut data: *mut u8 = std::ptr::null_mut();
                    let mut got = 0u32;
                    let mut flags = 0u32;
                    self.reader
                        .GetBuffer(&mut data, &mut got, &mut flags, None, None)
                        .map_err(|e| format!("the microphone stopped: {e}"))?;
                    let bytes = got as usize * PCM_BLOCK;
                    if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                        out.resize(out.len() + bytes, 0);
                    } else {
                        out.extend_from_slice(std::slice::from_raw_parts(data, bytes));
                    }
                    self.reader
                        .ReleaseBuffer(got)
                        .map_err(|e| format!("the microphone stopped: {e}"))?;
                }
            }
            Ok(out)
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            unsafe {
                let _ = self.client.Stop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, name: &str, default: bool) -> Input {
        Input { id: id.into(), name: name.into(), default }
    }

    fn mics() -> Vec<Input> {
        vec![
            input("loop", "Microsoft Teams Audio", false),
            input("builtin", "MacBook Pro Microphone", true),
            input("usb:1", "Jabra Speak 710", false),
        ]
    }

    #[test]
    fn switched_off_means_silence_whatever_is_connected() {
        let settings = Settings { mic: false, mic_device: "usb:1".into(), ..Settings::default() };
        assert_eq!(choose(&settings, &mics()), Choice::default());
        assert_eq!(choose(&settings, &[]), Choice::default());
    }

    #[test]
    fn the_system_default_is_taken_and_followed_as_such() {
        let settings = Settings::default();
        let choice = choose(&settings, &mics());
        assert_eq!(choice.input.as_ref().map(|i| i.id.as_str()), Some("builtin"));
        assert!(choice.default);
        assert_eq!(choice.issue, None);
        // No default flagged (pactl without get-default-source): the first one.
        let unflagged: Vec<Input> = mics().into_iter().map(|i| Input { default: false, ..i }).collect();
        let choice = choose(&settings, &unflagged);
        assert_eq!(choice.input.as_ref().map(|i| i.id.as_str()), Some("loop"));
        assert!(choice.default);
    }

    #[test]
    fn a_chosen_microphone_is_taken_by_id_and_stood_in_for_when_unplugged() {
        let settings = Settings { mic_device: "usb:1".into(), mic_device_name: "Jabra Speak 710".into(), ..Settings::default() };
        let choice = choose(&settings, &mics());
        assert_eq!(choice.input, Some(input("usb:1", "Jabra Speak 710", false)));
        assert!(!choice.default);
        assert_eq!(choice.issue, None);

        let without_jabra: Vec<Input> = mics().into_iter().filter(|i| i.id != "usb:1").collect();
        let choice = choose(&settings, &without_jabra);
        assert_eq!(choice.input.as_ref().map(|i| i.id.as_str()), Some("builtin"));
        assert!(choice.default, "the stand-in is the default, and followed as such");
        assert_eq!(choice.issue.as_deref(), Some("Jabra Speak 710 is not connected; recording with MacBook Pro Microphone."));

        // A chosen id without a remembered name still gets a sentence.
        let nameless = Settings { mic_device: "gone".into(), ..Settings::default() };
        let choice = choose(&nameless, &mics());
        assert_eq!(choice.issue.as_deref(), Some("The chosen microphone is not connected; recording with MacBook Pro Microphone."));
    }

    #[test]
    fn no_microphone_at_all_means_a_silent_recording_that_says_so() {
        let choice = choose(&Settings::default(), &[]);
        assert_eq!(choice.input, None);
        assert_eq!(choice.issue.as_deref(), Some(NO_MIC));
        let mut choice = choose(&Settings::default(), &mics());
        choice.mute("Microphone access is off; recording without sound.".into());
        assert_eq!(choice.input, None);
        assert!(!choice.default);
        assert!(choice.issue.unwrap().contains("access is off"));
    }

    #[test]
    fn inputs_are_listed_without_prompting_and_look_sane() {
        // Whatever this machine has: ids are unique, at most one default,
        // every entry has a name. (Windows and Linux builds are checked on
        // their own platforms; here this runs the macOS listing.)
        let list = inputs();
        let mut ids: Vec<&str> = list.iter().map(|i| i.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), list.len(), "{list:?}");
        assert!(list.iter().filter(|i| i.default).count() <= 1, "{list:?}");
        assert!(list.iter().all(|i| !i.name.is_empty() && !i.id.is_empty()), "{list:?}");
        let json = serde_json::to_value(&list).unwrap();
        if let Some(first) = json.as_array().and_then(|a| a.first()) {
            assert!(first.get("id").is_some() && first.get("name").is_some() && first.get("default").is_some());
        }
    }

    #[test]
    fn pactl_sources_are_parsed_and_monitors_left_out() {
        let text = "\
Source #0
\tState: SUSPENDED
\tName: alsa_output.pci-0000_00_1f.3.analog-stereo.monitor
\tDescription: Monitor of Built-in Audio Analog Stereo
\tMonitor of Sink: alsa_output.pci-0000_00_1f.3.analog-stereo

Source #1
\tState: RUNNING
\tName: alsa_input.pci-0000_00_1f.3.analog-stereo
\tDescription: Built-in Audio Analog Stereo
\tMonitor of Sink: n/a

Source #7
\tName: alsa_input.usb-Jabra_SPEAK_710-00.mono-fallback
\tDescription: Jabra SPEAK 710 Mono
";
        let list = pulse::parse_sources(text, "alsa_input.pci-0000_00_1f.3.analog-stereo");
        assert_eq!(
            list,
            vec![
                input("alsa_input.pci-0000_00_1f.3.analog-stereo", "Built-in Audio Analog Stereo", true),
                input("alsa_input.usb-Jabra_SPEAK_710-00.mono-fallback", "Jabra SPEAK 710 Mono", false),
            ]
        );
        // No description: the name stands in. No default known: none flagged.
        let bare = "Source #3\n\tName: pipewire.input\n";
        assert_eq!(pulse::parse_sources(bare, ""), vec![input("pipewire.input", "pipewire.input", false)]);
        assert_eq!(pulse::parse_sources("", "x"), vec![]);
        assert_eq!(
            pulse::default_from_info("Server Name: PulseAudio\nDefault Sink: alsa_output.x\nDefault Source: alsa_input.y\n"),
            "alsa_input.y"
        );
        assert_eq!(pulse::default_from_info("Server Name: PulseAudio\n"), "");
        // Not on Linux, `pactl` is usually absent: the listing is then empty
        // rather than an error.
        if !cfg!(target_os = "linux") {
            let _ = pulse::inputs();
        }
    }
}
