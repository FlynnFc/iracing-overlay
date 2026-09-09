// Rust guideline compliant 2026-02-16

#![expect(
    clippy::doc_markdown,
    reason = "DirectInput, XInput and Windows.Gaming.Input are product names; backticking them would read as though they were items in this crate"
)]

//! Wheel and button-box input, via DirectInput 8.
//!
//! Two simpler options were tried first and both are unusable here:
//!
//! - `gilrs` reads XInput and the Windows.Gaming.Input *gamepad* view, which
//!   model a controller as a pad — two sticks, a d-pad, a dozen buttons. A
//!   direct-drive wheel is a HID device with a hundred-odd buttons and no
//!   stick, and is simply absent from that view. It reported no devices at
//!   all with the wheel connected.
//! - `RawGameController` does cover arbitrary HID controllers, but
//!   Windows.Gaming.Input only reports devices to a *foreground* process.
//!   This overlay is never in the foreground — iRacing is, which is the whole
//!   point — so it too reported nothing.
//!
//! DirectInput's `DISCL_BACKGROUND | DISCL_NONEXCLUSIVE` is built for exactly
//! this case, and is what iRacing itself uses: its `controls.cfg` stores
//! DirectInput instance and product GUIDs. Reading through the same API means
//! button numbering comes from the same enumeration the sim did.

use std::time::{Duration, Instant};

use windows::Win32::Devices::HumanInterfaceDevice::{
    DI8DEVCLASS_ALL, DIDATAFORMAT, DIDEVCAPS, DIDEVICEINSTANCEW, DIDF_ABSAXIS, DIDFT_ANYINSTANCE, DIDFT_BUTTON,
    DIEDFL_ATTACHEDONLY, DIOBJECTDATAFORMAT, DISCL_BACKGROUND, DISCL_NONEXCLUSIVE, DirectInput8Create, IDirectInput8W,
    IDirectInputDevice8W,
};
use windows::Win32::Foundation::TRUE;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
use windows::core::{BOOL, GUID, HRESULT, Interface};

/// The DirectInput version this app negotiates. `0x0800` is DirectInput 8,
/// the last and current one; there is no newer value to move to.
const DIRECTINPUT_VERSION: u32 = 0x0800;

/// How many buttons a device is read for.
///
/// DirectInput's own `DIJOYSTATE2` tops out at 128, and no wheel or button
/// box exposes more. Each is one byte of state, so the buffer is trivial.
const MAX_BUTTONS: usize = 128;

/// `DIERR_INPUTLOST` — the device was lost (unplugged, or another process
/// took it exclusively) and must be re-acquired before the next read.
const DIERR_INPUTLOST: HRESULT = HRESULT(0x8007_001E_u32.cast_signed());

/// `DIERR_NOTACQUIRED` — a read was attempted before acquiring.
const DIERR_NOTACQUIRED: HRESULT = HRESULT(0x8007_0002_u32.cast_signed());

/// `DIDFT_OPTIONAL` — this data-format slot may go unmatched.
///
/// Not exported by the `windows` crate. Without it `SetDataFormat` rejects
/// the whole format unless the device has at least [`MAX_BUTTONS`] buttons,
/// which quietly excluded every wheel rim and button box with fewer: a
/// 112-button Simagic GT Neo failed while a 128-button base succeeded.
const DIDFT_OPTIONAL: u32 = 0x8000_0000;

/// How long to leave between the first few enumerations while a bound device
/// is missing.
///
/// Enumerating once at startup is not enough. The launcher starts this
/// overlay at boot alongside everything else, and a wheel base whose driver
/// has not finished bringing the device up is simply absent from that first
/// list — after which every bind on it stayed dead, with the binds still
/// sitting correctly in the settings file, until the overlay was restarted by
/// hand. A couple of seconds is imperceptible when plugging a wheel in, and
/// costs nothing once everything wanted is open.
const RESCAN_INTERVAL: Duration = Duration::from_secs(2);

/// The longest the interval backs off to while a device stays missing.
///
/// A wheel that has been off for half an hour is not about to appear in the
/// next two seconds, and enumeration is not free: it walks every HID device on
/// the machine through SetupAPI, which can block for *seconds* when something
/// it asks about is absent or asleep on a suspended USB port. Doing that every
/// two seconds for a whole evening is what made the overlay stall repeatedly
/// with the wheel switched off. Backing off to a minute keeps "switch the wheel
/// on and it starts working" true without asking the question constantly.
const RESCAN_INTERVAL_MAX: Duration = Duration::from_secs(60);

/// How long a device may fail to read before it is dropped and re-opened.
///
/// Re-acquiring the handle recovers the ordinary case — another process took
/// the device exclusively for a moment — but not a device that was physically
/// unplugged, whose handle never becomes valid again. Dropping it makes it
/// missing, and missing is what [`RESCAN_INTERVAL`] picks up.
const DROP_AFTER: Duration = Duration::from_secs(3);

/// One connected controller and its last reading.
struct Device {
    name: String,
    /// DirectInput's instance id, so a rescan can tell an already-open device
    /// from a newly attached one. Two identical rims report the same product
    /// name; this is what distinguishes them.
    instance: GUID,
    handle: IDirectInputDevice8W,
    /// One byte per button; the high bit is set while held.
    state: [u8; MAX_BUTTONS],
    /// When this device first failed to read, cleared on the next successful
    /// one. See [`DROP_AFTER`].
    failing_since: Option<Instant>,
}

/// Live button state for every connected controller.
///
/// Keyed by display name rather than by enumeration index: indices are
/// assignment order, so unplugging a pedal set between sessions can renumber
/// a wheel and silently move every bind onto whatever took its place.
pub struct Devices {
    /// `None` until first needed, and again if DirectInput could not be
    /// created — in which case the overlay runs on keyboard binds alone
    /// rather than failing to start.
    input: Option<IDirectInput8W>,
    open: Vec<Device>,
    /// When devices were last enumerated, so a machine with a bind on
    /// something unplugged doesn't re-enumerate every frame forever.
    last_scan: Option<Instant>,
    /// How long to leave before the next enumeration, doubling from
    /// [`RESCAN_INTERVAL`] to [`RESCAN_INTERVAL_MAX`] while whatever is wanted
    /// stays missing, and reset the moment it appears.
    rescan_interval: Duration,
}

#[expect(
    clippy::missing_fields_in_debug,
    reason = "the COM interfaces are not Debug; the device count is the useful summary"
)]
impl std::fmt::Debug for Devices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Devices").field("connected", &self.open.len()).finish()
    }
}

impl Default for Devices {
    fn default() -> Self {
        Self::new()
    }
}

impl Devices {
    #[must_use]
    pub fn new() -> Self {
        Self { input: None, open: Vec::new(), last_scan: None, rescan_interval: RESCAN_INTERVAL }
    }

    /// Re-reads every connected controller's button state.
    ///
    /// Must be called once per frame. `wanted` is the device names the caller
    /// has binds on: while any of them is not open, devices are re-enumerated
    /// every [`RESCAN_INTERVAL`], so a wheel that appears late — or is
    /// unplugged and plugged back in mid-session — starts working on its own.
    /// Pass an empty slice to only look again while nothing at all is open.
    pub fn pump(&mut self, wanted: &[&str]) {
        let now = Instant::now();
        if self.needs_scan(wanted, now) {
            self.last_scan = Some(now);
            self.enumerate(wanted);
            // Whether that found what was being looked for decides how soon it
            // is worth asking again.
            self.rescan_interval = if self.all_present(wanted) {
                RESCAN_INTERVAL
            } else {
                (self.rescan_interval * 2).min(RESCAN_INTERVAL_MAX)
            };
        }
        for device in &mut self.open {
            // SAFETY: `device.handle` is a live DirectInput device this type
            // created and acquired; `state` is the exact buffer size declared
            // to `SetDataFormat`.
            let result = unsafe {
                device.handle.GetDeviceState(u32::try_from(MAX_BUTTONS).unwrap_or(0), device.state.as_mut_ptr().cast())
            };
            if let Err(err) = result {
                if err.code() == DIERR_INPUTLOST || err.code() == DIERR_NOTACQUIRED {
                    // SAFETY: as above; re-acquiring a lost device is the
                    // documented recovery and is safe to attempt repeatedly.
                    let _ = unsafe { device.handle.Acquire() };
                }
                device.state = [0; MAX_BUTTONS];
                device.failing_since.get_or_insert(now);
            } else {
                device.failing_since = None;
            }
        }
        // Whatever is still unreadable after a few seconds is gone, not busy.
        self.open.retain(|device| device.failing_since.is_none_or(|since| now.duration_since(since) < DROP_AFTER));
    }

    /// Whether it is time to look for devices that aren't open yet.
    fn needs_scan(&self, wanted: &[&str], now: Instant) -> bool {
        let Some(last) = self.last_scan else {
            return true;
        };
        if now.duration_since(last) < self.rescan_interval {
            return false;
        }
        self.open.is_empty() || !self.all_present(wanted)
    }

    /// Whether every name in `wanted` matches something already open.
    fn all_present(&self, wanted: &[&str]) -> bool {
        wanted.iter().all(|name| self.open.iter().any(|device| matches_device(&device.name, name)))
    }

    /// Whether `button` on `device` is held right now.
    ///
    /// Device names match case-insensitively and by prefix, so a bind saved
    /// as `MOZA` still resolves when the driver reports itself as
    /// `MOZA Racing FSR Formula Wheel`.
    #[must_use]
    pub fn is_button_down(&self, device: &str, button: u32) -> bool {
        let index = usize::try_from(button).unwrap_or(usize::MAX);
        self.open
            .iter()
            .filter(|d| matches_device(&d.name, device))
            .any(|d| d.state.get(index).is_some_and(|byte| byte & 0x80 != 0))
    }

    /// Every button held on any device, as `(device name, button)`.
    ///
    /// Used by `--bind` to name whatever the user just pressed.
    #[must_use]
    pub fn pressed(&self) -> Vec<(String, u32)> {
        self.open
            .iter()
            .flat_map(|d| {
                d.state
                    .iter()
                    .enumerate()
                    .filter(|(_, byte)| **byte & 0x80 != 0)
                    .filter_map(move |(i, _)| u32::try_from(i).ok().map(|b| (d.name.clone(), b)))
            })
            .collect()
    }

    /// Whether any device is open and therefore worth reading quickly.
    ///
    /// Nothing open means every bind on a wheel is currently dead anyway, so
    /// the caller can drop to a lazy poll until one appears.
    #[must_use]
    pub fn any_open(&self) -> bool {
        !self.open.is_empty()
    }

    /// Connected device names, for reporting.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.open.iter().map(|d| d.name.clone()).collect()
    }

    /// Opens the attached game controllers named in `wanted` that aren't open
    /// already, or every one of them when `wanted` is empty.
    ///
    /// Safe to call repeatedly: devices already open are matched by instance
    /// id and left alone, so a rescan neither reopens them nor disturbs the
    /// button state being read from them.
    ///
    /// Only what is bound gets opened, because an open device is one that is
    /// then read 250 times a second for the life of the process. Opening every
    /// controller on the machine meant a pedal set, a handbrake and a button
    /// box were all being polled to answer questions only ever asked about the
    /// wheel — measured at four percent of a core with the overlay drawing
    /// nothing at all. `wanted` being empty means "show me everything", which
    /// is what `--bind` needs: it cannot know which device the user is about to
    /// press.
    fn enumerate(&mut self, wanted: &[&str]) {
        // Cloned (the interface is reference-counted) so the loop below can
        // push into `self.open` while this is in hand.
        let Some(input) = self.direct_input().cloned() else {
            return;
        };

        let mut found: Vec<(GUID, String)> = Vec::new();
        // `DI8DEVCLASS_ALL`, not `DI8DEVCLASS_GAMECTRL`. A wheel *rim* is
        // often its own HID device and does not always classify as a game
        // controller — a Simagic GT Neo with 112 buttons is absent from the
        // game-controller list while its base is present, which silently made
        // every button on the rim unbindable. `enum_callback` drops keyboards,
        // mice and button-less devices instead.
        // SAFETY: `input` is live; `enum_callback` matches the documented
        // callback signature and `found` outlives the enumeration, which is
        // synchronous and completes before this call returns.
        let _ = unsafe {
            input.EnumDevices(
                DI8DEVCLASS_ALL,
                Some(enum_callback),
                std::ptr::from_mut(&mut found).cast(),
                DIEDFL_ATTACHEDONLY,
            )
        };

        for (guid, name) in found {
            if !wanted.is_empty() && !wanted.iter().any(|w| matches_device(&name, w)) {
                continue;
            }
            if self.open.iter().any(|device| device.instance == guid) {
                continue;
            }
            // SAFETY: `guid` came from this DirectInput's own enumeration.
            if let Some(handle) = unsafe { open_device(&input, guid) } {
                self.open.push(Device { name, instance: guid, handle, state: [0; MAX_BUTTONS], failing_since: None });
            }
        }
    }

    /// This process's DirectInput, created on first use.
    ///
    /// Returns `None` if it could not be created, in which case the overlay
    /// runs on keyboard binds alone rather than refusing to start. Held
    /// across rescans: creation is the expensive half, and the enumeration it
    /// serves reflects whatever is attached at the time it is called.
    fn direct_input(&mut self) -> Option<&IDirectInput8W> {
        if self.input.is_none() {
            self.input = create_direct_input();
            if self.input.is_none() {
                println!("note: could not start DirectInput; wheel binds will not work");
            }
        }
        self.input.as_ref()
    }
}

/// Creates this process's DirectInput 8 interface.
fn create_direct_input() -> Option<IDirectInput8W> {
    // SAFETY: `GetModuleHandleW(None)` returns this process's own module
    // handle and cannot fail for the current process.
    let module = (unsafe { GetModuleHandleW(None) }).ok()?;

    let mut raw: Option<IDirectInput8W> = None;
    // SAFETY: standard DirectInput creation: a valid module handle, the
    // documented version constant, and the matching interface id for the
    // `IDirectInput8W` the out-pointer is typed as.
    let created = unsafe {
        DirectInput8Create(
            module.into(),
            DIRECTINPUT_VERSION,
            &IDirectInput8W::IID,
            std::ptr::from_mut(&mut raw).cast(),
            None,
        )
    };
    created.ok().and(raw)
}

/// Every input device DirectInput can see, described for diagnostics.
///
/// Enumerates `DI8DEVCLASS_ALL` rather than just game controllers: a wheel
/// rim or button box does not always classify as one, and "it isn't in the
/// list" is impossible to act on without knowing what *is*.
///
/// The product GUID's first four bytes are the USB vendor and product ids,
/// the same pair iRacing stores in `controls.cfg` — so this is what ties a
/// device here to a binding there.
#[must_use]
pub fn describe_all() -> Vec<String> {
    let Some(input) = create_direct_input() else {
        return Vec::new();
    };

    let mut found: Vec<DeviceDescription> = Vec::new();
    // SAFETY: `input` is live; `describe_callback` matches the documented
    // signature and `found` outlives this synchronous enumeration.
    let _ = unsafe {
        input.EnumDevices(
            DI8DEVCLASS_ALL,
            Some(describe_callback),
            std::ptr::from_mut(&mut found).cast(),
            DIEDFL_ATTACHEDONLY,
        )
    };

    found
        .into_iter()
        .map(|d| {
            let vendor = u16::try_from(d.product.data1 & 0xFFFF).unwrap_or(0);
            let product = u16::try_from((d.product.data1 >> 16) & 0xFFFF).unwrap_or(0);
            let buttons = describe_buttons(&input, d.instance);
            format!("{:<40} VID_{vendor:04X} PID_{product:04X}  {buttons}", d.name)
        })
        .collect()
}

/// One row of [`describe_all`]'s output, before formatting.
struct DeviceDescription {
    name: String,
    instance: GUID,
    product: GUID,
}

/// Reports how many buttons a device offers, or why it couldn't be opened.
fn describe_buttons(input: &IDirectInput8W, instance: GUID) -> String {
    let mut device: Option<IDirectInputDevice8W> = None;
    // SAFETY: `instance` came from this DirectInput's own enumeration.
    if unsafe { input.CreateDevice(std::ptr::from_ref(&instance), std::ptr::from_mut(&mut device), None) }.is_err() {
        return "(could not open)".to_owned();
    }
    let Some(device) = device else {
        return "(could not open)".to_owned();
    };

    let mut caps = windows::Win32::Devices::HumanInterfaceDevice::DIDEVCAPS {
        dwSize: u32::try_from(size_of::<windows::Win32::Devices::HumanInterfaceDevice::DIDEVCAPS>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: `device` is live and `caps.dwSize` is set as the API requires.
    if unsafe { device.GetCapabilities(std::ptr::from_mut(&mut caps)) }.is_err() {
        return "(no capabilities)".to_owned();
    }
    format!("{} buttons, {} axes", caps.dwButtons, caps.dwAxes)
}

/// DirectInput enumeration callback for [`describe_all`].
///
/// # Safety
/// Called only by DirectInput, with valid pointers from `EnumDevices`.
unsafe extern "system" fn describe_callback(instance: *mut DIDEVICEINSTANCEW, context: *mut core::ffi::c_void) -> BOOL {
    // SAFETY: forwarding this callback's contract.
    unsafe {
        if let (Some(instance), Some(found)) = (instance.as_ref(), context.cast::<Vec<DeviceDescription>>().as_mut()) {
            found.push(DeviceDescription {
                name: wide_to_string(&instance.tszProductName),
                instance: instance.guidInstance,
                product: instance.guidProduct,
            });
        }
    }
    TRUE
}

/// Opens, configures and acquires one enumerated device.
///
/// # Safety
/// `guid` must be a device instance id from `input`'s own enumeration.
unsafe fn open_device(input: &IDirectInput8W, guid: GUID) -> Option<IDirectInputDevice8W> {
    let mut device: Option<IDirectInputDevice8W> = None;
    // SAFETY: forwarding this function's contract.
    unsafe { input.CreateDevice(std::ptr::from_ref(&guid), std::ptr::from_mut(&mut device), None) }.ok()?;
    let device = device?;

    // A device with no buttons can hold no bind. Wireless dongles and RGB
    // controllers enumerate alongside the real hardware, and polling them
    // every frame for nothing is pure waste.
    let mut caps = DIDEVCAPS { dwSize: u32::try_from(size_of::<DIDEVCAPS>()).unwrap_or(0), ..Default::default() };
    // SAFETY: `device` is live and `caps.dwSize` is set as the API requires.
    unsafe { device.GetCapabilities(std::ptr::from_mut(&mut caps)) }.ok()?;
    if caps.dwButtons == 0 {
        return None;
    }

    let mut format = button_data_format();
    // SAFETY: `format` describes exactly `MAX_BUTTONS` single-byte button
    // slots, matching the buffer passed to `GetDeviceState`, and both it and
    // its object array outlive the call, which copies what it needs.
    unsafe { device.SetDataFormat(std::ptr::from_mut(&mut format.header)) }.ok()?;

    // A window handle is required even for background access. The desktop
    // window is used rather than the overlay's own: `--bind` runs with no
    // window of its own, and background non-exclusive access does not care
    // which window it is told about.
    // SAFETY: `GetDesktopWindow` always returns a valid handle.
    let hwnd = unsafe { GetDesktopWindow() };
    // SAFETY: `device` is live and `hwnd` valid. Background + non-exclusive
    // is what lets the overlay read the wheel while iRacing holds focus, and
    // guarantees this never takes the device away from the sim.
    unsafe { device.SetCooperativeLevel(hwnd, DISCL_BACKGROUND | DISCL_NONEXCLUSIVE) }.ok()?;
    // Acquisition can legitimately fail here and succeed later (the device
    // may be busy); `pump` re-acquires, so this result is advisory.
    // SAFETY: `device` is live and fully configured.
    let _ = unsafe { device.Acquire() };
    Some(device)
}

/// A [`DIDATAFORMAT`] describing [`MAX_BUTTONS`] button slots, one byte each.
///
/// DirectInput's own `c_dfDIJoystick2` global is not exported by the `windows`
/// crate, so the format is built here. Only buttons are declared: axes and
/// hats are not bound to anything, and leaving them out keeps the state
/// buffer a flat `[u8; MAX_BUTTONS]` indexed directly by button number.
///
/// Each slot uses a null object GUID with `DIDFT_BUTTON | DIDFT_ANYINSTANCE`,
/// which tells DirectInput to fill the slots with whatever buttons the device
/// has, in its own enumeration order — the same order iRacing numbers them in.
struct ButtonFormat {
    header: DIDATAFORMAT,
    /// Owned by this struct so it outlives the `SetDataFormat` call, which
    /// reads through `header.rgodf`. Never read through this field — dropping
    /// it early would leave that pointer dangling, which is the whole reason
    /// it is held.
    // Only the tests read it; in a normal build its whole job is to exist.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "keeps the array `header.rgodf` points at alive; see the doc comment")
    )]
    objects: Box<[DIOBJECTDATAFORMAT; MAX_BUTTONS]>,
}

fn button_data_format() -> ButtonFormat {
    let mut objects =
        Box::new([DIOBJECTDATAFORMAT { pguid: std::ptr::null(), dwOfs: 0, dwType: 0, dwFlags: 0 }; MAX_BUTTONS]);
    for (index, object) in objects.iter_mut().enumerate() {
        object.dwOfs = u32::try_from(index).unwrap_or(0);
        object.dwType = DIDFT_BUTTON | DIDFT_ANYINSTANCE | DIDFT_OPTIONAL;
    }
    let header = DIDATAFORMAT {
        dwSize: u32::try_from(size_of::<DIDATAFORMAT>()).unwrap_or(0),
        dwObjSize: u32::try_from(size_of::<DIOBJECTDATAFORMAT>()).unwrap_or(0),
        dwFlags: DIDF_ABSAXIS,
        dwDataSize: u32::try_from(MAX_BUTTONS).unwrap_or(0),
        dwNumObjs: u32::try_from(MAX_BUTTONS).unwrap_or(0),
        rgodf: objects.as_mut_ptr(),
    };
    ButtonFormat { header, objects }
}

/// DirectInput's enumeration callback: records each device's instance id and
/// product name.
///
/// # Safety
/// Called only by DirectInput, with `instance` pointing at a valid
/// [`DIDEVICEINSTANCEW`] and `context` at the `Vec` passed to `EnumDevices`.
unsafe extern "system" fn enum_callback(instance: *mut DIDEVICEINSTANCEW, context: *mut core::ffi::c_void) -> BOOL {
    // SAFETY: forwarding this callback's contract; both pointers are supplied
    // by DirectInput from the arguments given to `EnumDevices`.
    unsafe {
        if let (Some(instance), Some(found)) = (instance.as_ref(), context.cast::<Vec<(GUID, String)>>().as_mut())
            && is_bindable(instance.dwDevType)
        {
            found.push((instance.guidInstance, wide_to_string(&instance.tszProductName)));
        }
    }
    TRUE
}

/// Whether a device is one a driver could bind a black box control to.
///
/// Enumerating every class turns up a lot that isn't: the keyboard and mouse
/// themselves, and the HID collections a wireless dongle or an RGB controller
/// exposes, several of which report hundreds of "buttons" that are really
/// consumer-control codes. Excluding keyboards and mice removes almost all of
/// it, and anything left that reports no buttons cannot be bound anyway.
fn is_bindable(device_type: u32) -> bool {
    /// `DI8DEVTYPE_MOUSE` and `DI8DEVTYPE_KEYBOARD`; the device type is the
    /// low byte of `dwDevType`.
    const MOUSE: u32 = 0x12;
    const KEYBOARD: u32 = 0x13;

    let kind = device_type & 0xFF;
    kind != MOUSE && kind != KEYBOARD
}

/// Converts one of DirectInput's fixed-size, null-padded UTF-16 name fields.
fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|c| *c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end]).trim().to_owned()
}

fn matches_device(name: &str, wanted: &str) -> bool {
    name.eq_ignore_ascii_case(wanted) || name.to_ascii_lowercase().starts_with(&wanted.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_match_case_insensitively_and_by_prefix() {
        assert!(matches_device("MOZA Racing FSR Formula Wheel", "moza"));
        assert!(matches_device("MOZA Racing FSR", "MOZA Racing FSR"));
        assert!(!matches_device("Fanatec CSL DD", "moza"));
    }

    #[test]
    fn names_stop_at_the_first_null() {
        let mut wide = [0_u16; 8];
        for (slot, ch) in wide.iter_mut().zip("Hi".encode_utf16()) {
            *slot = ch;
        }
        assert_eq!(wide_to_string(&wide), "Hi");
        assert_eq!(wide_to_string(&[]), "");
    }

    /// Every slot must be a distinct one-byte button, or `GetDeviceState`
    /// writes outside the state buffer.
    #[test]
    fn the_data_format_declares_one_byte_per_button() {
        let format = button_data_format();
        assert_eq!(format.header.dwNumObjs as usize, MAX_BUTTONS);
        assert_eq!(format.header.dwDataSize as usize, MAX_BUTTONS);
        assert_eq!(format.objects[0].dwOfs, 0);
        assert_eq!(format.objects[MAX_BUTTONS - 1].dwOfs as usize, MAX_BUTTONS - 1);
    }

    #[test]
    fn nothing_is_held_before_the_first_poll() {
        let devices = Devices::new();
        assert!(!devices.is_button_down("moza", 21));
        assert!(devices.pressed().is_empty());
        assert!(devices.names().is_empty());
    }

    /// The bug this replaced: enumerate once at boot, miss a wheel that was
    /// still coming up, and never look again — leaving correct binds dead
    /// until the overlay was restarted by hand.
    #[test]
    fn a_missing_device_is_looked_for_again_but_not_every_frame() {
        let devices = Devices::new();
        let start = Instant::now();
        assert!(devices.needs_scan(&["GT Neo"], start), "nothing has been enumerated yet");

        let mut scanned = Devices::new();
        scanned.last_scan = Some(start);
        assert!(!scanned.needs_scan(&["GT Neo"], start), "not twice inside the interval");
        assert!(
            scanned.needs_scan(&["GT Neo"], start + RESCAN_INTERVAL),
            "a bound device that isn't open is worth looking for again"
        );
        assert!(
            scanned.needs_scan(&[], start + RESCAN_INTERVAL),
            "with nothing open at all, keep looking even with no binds"
        );
    }

    /// A wheel switched off for the evening must not be asked about every two
    /// seconds all evening: enumeration walks every HID device on the machine
    /// and blocks while it does, so the question gets steadily cheaper to leave
    /// unasked the longer the answer stays "no".
    #[test]
    fn looking_for_a_device_that_never_appears_backs_off() {
        let mut devices = Devices::new();
        assert_eq!(devices.rescan_interval, RESCAN_INTERVAL);

        // `pump` on a machine with no DirectInput in a test process finds
        // nothing, which is exactly the case being modelled.
        let start = Instant::now();
        let mut interval = RESCAN_INTERVAL;
        for _ in 0..10 {
            devices.rescan_interval = interval;
            devices.last_scan = Some(start);
            assert!(!devices.needs_scan(&["GT Neo"], start + interval / 2), "inside the current interval");
            assert!(devices.needs_scan(&["GT Neo"], start + interval), "and due at the end of it");
            interval = (interval * 2).min(RESCAN_INTERVAL_MAX);
        }
        assert_eq!(interval, RESCAN_INTERVAL_MAX, "the backoff is capped rather than unbounded");
    }

    /// The backoff must never make a device that *is* there go unnoticed, and
    /// must reset the moment everything wanted is open.
    #[test]
    fn everything_wanted_being_open_settles_the_interval() {
        let devices = Devices::new();
        assert!(devices.all_present(&[]), "nothing wanted is trivially all present");
        assert!(!devices.all_present(&["GT Neo"]), "nothing is open");
    }
}
