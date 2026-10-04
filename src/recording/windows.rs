//! Windows shared-mode adapter. COM packets are copied/released on their owning thread.
use super::{Capture, Input, Packet};
use crate::{Result, error};
use ::windows::{
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_TIMEOUT},
        Media::Audio::*,
        System::{
            Com::{StructuredStorage::*, *},
            Threading::*,
            Variant::VT_BLOB,
        },
    },
    core::*,
};
use serde_json::{Value, json};
use std::{
    mem::ManuallyDrop,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn failed(e: ::windows::core::Error) -> crate::Error {
    let code = if e.code() == AUDCLNT_E_DEVICE_INVALIDATED
        || e.code() == AUDCLNT_E_RESOURCES_INVALIDATED
    {
        "CAPTURE_DEVICE_CHANGED"
    } else {
        "CAPTURE_DEVICE_ERROR"
    };
    error(
        code,
        format!(
            "Windows audio: {} ({:#010x})",
            e.message(),
            e.code().0 as u32
        ),
    )
}
struct Apartment;
impl Apartment {
    fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(failed)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct Memory(*mut std::ffi::c_void);
impl Drop for Memory {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.0));
        }
    }
}
unsafe fn text(pointer: PWSTR) -> Result<String> {
    let _memory = Memory(pointer.0.cast());
    unsafe {
        pointer
            .to_string()
            .map_err(|e| error("CAPTURE_DEVICE_ERROR", e.to_string()))
    }
}
fn enumerator() -> Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(failed) }
}
fn endpoint(id: &str) -> Result<IMMDevice> {
    let id: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let device = enumerator()?
            .GetDevice(PCWSTR(id.as_ptr()))
            .map_err(failed)?;
        let flow: IMMEndpoint = device.cast().map_err(failed)?;
        if flow.GetDataFlow().map_err(failed)? != eCapture
            || device.GetState().map_err(failed)? != DEVICE_STATE_ACTIVE
        {
            return Err(error(
                "CAPTURE_DEVICE_CHANGED",
                "Selected input is not an active capture endpoint",
            ));
        }
        Ok(device)
    }
}
fn format(client: &IAudioClient) -> Result<Value> {
    unsafe {
        let pointer = client.GetMixFormat().map_err(failed)?;
        let _memory = Memory(pointer.cast());
        let f = pointer.read_unaligned();
        let (tag, ch, rate, bits, align, extra) = (
            f.wFormatTag,
            f.nChannels,
            f.nSamplesPerSec,
            f.wBitsPerSample,
            f.nBlockAlign,
            f.cbSize,
        );
        let mut result = json!({"format_tag":tag,"channels":ch,"sample_rate":rate,"bits":bits,"block_align":align,"extra_bytes":extra});
        if tag == 65534 && extra >= 22 {
            let f = pointer.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned();
            let (mask, valid, subtype) =
                (f.dwChannelMask, f.Samples.wValidBitsPerSample, f.SubFormat);
            result["speaker_mask"] = json!(mask);
            result["valid_bits"] = json!(valid);
            result["subtype"] = json!(format!("{subtype:?}"));
        }
        Ok(result)
    }
}
fn endpoint_description(device: &IMMDevice, client: &IAudioClient) -> Result<Value> {
    unsafe {
        let id = text(device.GetId().map_err(failed)?)?;
        let properties = device.OpenPropertyStore(STGM_READ).map_err(failed)?;
        let value = properties
            .GetValue(&PKEY_Device_FriendlyName)
            .map_err(failed)?;
        let name = text(PropVariantToStringAlloc(&value).map_err(failed)?)?;
        let (mut period, mut minimum) = (0, 0);
        client
            .GetDevicePeriod(Some(&mut period), Some(&mut minimum))
            .map_err(failed)?;
        Ok(
            json!({"type":"endpoint","id":id,"name":name,"mix_format":format(client)?,"period_100ns":period,"minimum_period_100ns":minimum}),
        )
    }
}
fn process(pid: u32) -> Result<(Handle, Value)> {
    unsafe {
        let handle = Handle(
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
            .map_err(failed)?,
        );
        if WaitForSingleObject(handle.0, 0) != WAIT_TIMEOUT {
            return Err(error(
                "CAPTURE_PROCESS_EXITED",
                "Selected process is not running",
            ));
        }
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user)
            .map_err(failed)?;
        let creation = (created.dwHighDateTime as u64) << 32 | created.dwLowDateTime as u64;
        Ok((
            handle,
            json!({"type":"process","pid":pid,"creation_filetime_100ns":creation.to_string(),"scope":"selected_process_and_descendants","device_sample_position":"unavailable; packet counts and QPC retained"}),
        ))
    }
}
pub(super) fn inputs() -> Result<Value> {
    let _apartment = Apartment::new()?;
    unsafe {
        let en = enumerator()?;
        let devices = en
            .EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)
            .map_err(failed)?;
        let count = devices.GetCount().map_err(failed)?;
        if count > 128 {
            return Err(error("LIMIT_EXCEEDED", "More than 128 active audio inputs"));
        }
        let mut result = Vec::new();
        for i in 0..count {
            let d = devices.Item(i).map_err(failed)?;
            let id = text(d.GetId().map_err(failed)?)?;
            let report = (|| {
                let c: IAudioClient = d.Activate(CLSCTX_ALL, None).map_err(failed)?;
                endpoint_description(&d, &c)
            })();
            result.push(match report {
                Ok(v) => v,
                Err(e) => json!({"id":id,"error":{"code":e.code,"message":e.message}}),
            });
        }
        Ok(
            json!({"inputs":result,"capture_started":false,"platform":"windows","process_loopback":"explicit PID; Windows build 20348 or newer"}),
        )
    }
}
pub(super) fn describe(input: &Input) -> Result<Value> {
    let _apartment = Apartment::new()?;
    match input {
        Input::Endpoint { id } => unsafe {
            let d = endpoint(id)?;
            let c: IAudioClient = d.Activate(CLSCTX_ALL, None).map_err(failed)?;
            endpoint_description(&d, &c)
        },
        Input::Process { pid } => Ok(process(*pid)?.1),
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Ready(Arc<AtomicBool>, Arc<ActivationArgs>);
impl IActivateAudioInterfaceCompletionHandler_Impl for Ready_Impl {
    fn ActivateCompleted(
        &self,
        _: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> ::windows::core::Result<()> {
        let _keep_parameters_alive = &self.1;
        self.0.store(true, Ordering::Release);
        Ok(())
    }
}
struct ActivationArgs {
    _params: Box<AUDIOCLIENT_ACTIVATION_PARAMS>,
    variant: ManuallyDrop<PROPVARIANT>,
}
// Immutable after construction; the only raw pointer targets the owned stable Box.
// The callback retains this allocation until Windows completes even after a timeout.
unsafe impl Send for ActivationArgs {}
unsafe impl Sync for ActivationArgs {}
fn activate_process(pid: u32) -> Result<IAudioClient> {
    let mut params = Box::new(AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    });
    // The blob borrows the owned Box, so PropVariantClear must not free its pointer.
    let variant = ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: std::mem::size_of_val(&*params) as u32,
                        pBlobData: (&mut *params as *mut AUDIOCLIENT_ACTIVATION_PARAMS).cast(),
                    },
                },
                ..Default::default()
            }),
        },
    });
    let args = Arc::new(ActivationArgs {
        _params: params,
        variant,
    });
    let ready = Arc::new(AtomicBool::new(false));
    let handler: IActivateAudioInterfaceCompletionHandler =
        Ready(ready.clone(), args.clone()).into();
    unsafe {
        let operation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&*args.variant),
            &handler,
        )
        .map_err(failed)?;
        let start = Instant::now();
        while !ready.load(Ordering::Acquire) {
            if start.elapsed() > Duration::from_secs(5) {
                return Err(error(
                    "CAPTURE_TIMEOUT",
                    "Process audio activation did not finish",
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut status = HRESULT(0);
        let mut object = None;
        operation
            .GetActivateResult(&mut status, &mut object)
            .map_err(failed)?;
        status.ok().map_err(failed)?;
        object
            .ok_or_else(|| error("CAPTURE_DEVICE_ERROR", "Activation returned no interface"))?
            .cast()
            .map_err(failed)
    }
}
struct Native {
    capture: IAudioCaptureClient,
    client: IAudioClient,
    endpoint: Option<IMMDevice>,
    process: Option<Handle>,
    report: Value,
    running: bool,
    _apartment: Apartment,
}
impl Capture for Native {
    fn next(&mut self) -> Result<Option<Packet>> {
        unsafe {
            if let Some(p) = &self.process
                && WaitForSingleObject(p.0, 0) != WAIT_TIMEOUT
            {
                return Err(error(
                    "CAPTURE_PROCESS_EXITED",
                    "Selected process exited during capture",
                ));
            }
            if let Some(d) = &self.endpoint
                && d.GetState().map_err(failed)? != DEVICE_STATE_ACTIVE
            {
                return Err(error(
                    "CAPTURE_DEVICE_CHANGED",
                    "Selected endpoint became unavailable",
                ));
            }
            let available = self.capture.GetNextPacketSize().map_err(failed)?;
            if available == 0 {
                return Ok(None);
            }
            let (mut data, mut frames, mut flags, mut position, mut qpc) =
                (std::ptr::null_mut(), 0, 0, 0, 0);
            self.capture
                .GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    Some(&mut position),
                    Some(&mut qpc),
                )
                .map_err(failed)?;
            let result = if frames == 0 || frames > 48_000 || frames != available {
                Err(error(
                    "INVALID_CAPTURE_PACKET",
                    "Native packet exceeds the one-second buffer bound",
                ))
            } else if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                Ok(vec![0; frames as usize * 4])
            } else if data.is_null() {
                Err(error(
                    "INVALID_CAPTURE_PACKET",
                    "Native packet contains no data",
                ))
            } else {
                Ok(std::slice::from_raw_parts(data, frames as usize * 4).to_vec())
            };
            // Release even rejected packets before returning any data/validation error.
            self.capture.ReleaseBuffer(frames).map_err(failed)?;
            Ok(Some(Packet {
                pcm: result?,
                frames,
                flags,
                device_position: position,
                qpc_100ns: qpc,
            }))
        }
    }
    fn stop(&mut self) -> Result<()> {
        if self.running {
            unsafe {
                self.client.Stop().map_err(failed)?;
            }
            self.running = false;
        }
        Ok(())
    }
    fn device_clock(&self) -> bool {
        self.endpoint.is_some()
    }
    fn description(&self) -> Value {
        self.report.clone()
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
pub(super) fn open(input: &Input) -> Result<Box<dyn Capture>> {
    let apartment = Apartment::new()?;
    unsafe {
        let (client, endpoint, process, mut report) = match input {
            Input::Endpoint { id } => {
                let d = endpoint(id)?;
                let c: IAudioClient = d.Activate(CLSCTX_ALL, None).map_err(failed)?;
                let r = endpoint_description(&d, &c)?;
                (c, Some(d), None, r)
            }
            Input::Process { pid } => {
                let (p, r) = process(*pid)?;
                (activate_process(*pid)?, None, Some(p), r)
            }
        };
        let fmt = WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: 2,
            nSamplesPerSec: 48000,
            nAvgBytesPerSec: 192000,
            nBlockAlign: 4,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let flags = if endpoint.is_some() {
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
        } else {
            AUDCLNT_STREAMFLAGS_LOOPBACK
        };
        client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &fmt, None)
            .map_err(failed)?;
        let capture: IAudioCaptureClient = client.GetService().map_err(failed)?;
        let size = client.GetBufferSize().map_err(failed)?;
        if size == 0 || size > 48000 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Native capture buffer exceeds one second",
            ));
        }
        report["buffer_frames"] = json!(size);
        report["reported_stream_latency_100ns"] = match client.GetStreamLatency() {
            Ok(v) => json!(v),
            Err(e) => json!({"unavailable":format!("{:#010x}",e.code().0 as u32)}),
        };
        report["requested_format"] = json!({"sample_rate":48000,"channels":2,"bits":16});
        let mut native = Native {
            capture,
            client,
            endpoint,
            process,
            report,
            running: false,
            _apartment: apartment,
        };
        native.client.Start().map_err(failed)?;
        native.running = true;
        Ok(Box::new(native))
    }
}
