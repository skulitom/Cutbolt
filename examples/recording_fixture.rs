//! Original synthetic WASAPI playback fixture for isolated recording acceptance.
//! Explicit invocation only. Own unique nonpersistent session; never changes device settings.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{
        io::{self, Write},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };
    use windows::Win32::{Media::Audio::*, System::Com::*};
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        return Err("Expected: seconds mute|quiet sample_rate tone_offset_hz".into());
    }
    let seconds = args[1].parse::<u64>()?;
    let muted = match args[2].as_str() {
        "mute" => true,
        "quiet" => false,
        _ => return Err("Use mute or quiet".into()),
    };
    let rate = args[3].parse::<u32>()?;
    let offset = args[4].parse::<u32>()?;
    if !(1..=7300).contains(&seconds) || ![44100, 48000].contains(&rate) || offset > 3000 {
        return Err("Fixture parameters outside limits".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = io::stdin().read_line(&mut line);
        stop_thread.store(true, Ordering::Release);
    });
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let endpoint = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = endpoint.Activate(CLSCTX_ALL, None)?;
        let format = WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: 2,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * 4,
            nBlockAlign: 4,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let guid = CoCreateGuid()?;
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
                | AUDCLNT_STREAMFLAGS_NOPERSIST,
            2_000_000,
            0,
            &format,
            Some(&guid),
        )?;
        let volume: ISimpleAudioVolume = client.GetService()?;
        volume.SetMasterVolume(1.0, std::ptr::null())?;
        volume.SetMute(muted, std::ptr::null())?;
        let writer: IAudioRenderClient = client.GetService()?;
        let size = client.GetBufferSize()?;
        let mut position = 0u64;
        let mut fill = |frames: u32| -> windows::core::Result<()> {
            let pointer = writer.GetBuffer(frames)?;
            let values = std::slice::from_raw_parts_mut(pointer.cast::<i16>(), frames as usize * 2);
            for v in values.as_chunks_mut::<2>().0 {
                let t = position as f64 / rate as f64;
                v[0] = (100.0 * (std::f64::consts::TAU * (310 + offset) as f64 * t).sin()).round()
                    as i16;
                v[1] = (50.0 * (std::f64::consts::TAU * (710 + offset) as f64 * t).sin()).round()
                    as i16;
                position += 1;
            }
            writer.ReleaseBuffer(frames, 0)
        };
        fill(size)?;
        client.Start()?;
        println!(
            "{}",
            serde_json::json!({"ready":true,"pid":std::process::id(),"rate":rate,"muted":muted,"session":format!("{guid:?}"),"frequencies":[310+offset,710+offset]})
        );
        io::stdout().flush()?;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(seconds) && !stop.load(Ordering::Acquire) {
            let available = size - client.GetCurrentPadding()?;
            if available > 0 {
                fill(available)?;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        client.Stop()?;
        println!(
            "{}",
            serde_json::json!({"stopped":true,"submitted_frames":position,"wall_seconds":start.elapsed().as_secs_f64()})
        );
        drop(writer);
        drop(volume);
        drop(client);
        drop(endpoint);
        drop(en);
        CoUninitialize();
    }
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("This native recording fixture requires Windows");
    std::process::exit(1);
}
