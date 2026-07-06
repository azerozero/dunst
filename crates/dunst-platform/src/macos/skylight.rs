use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::OnceLock;

#[repr(C)]
#[derive(Clone, Copy)]
struct Psn {
    high: u32,
    low: u32,
}

type GetFrontProcess = unsafe extern "C" fn(*mut Psn) -> i32;
type MainConnectionId = unsafe extern "C" fn() -> u32;
type GetWindowOwner = unsafe extern "C" fn(u32, u32, *mut u32) -> i32;
type GetConnectionPsn = unsafe extern "C" fn(u32, *mut Psn) -> i32;
type PostEventRecordTo = unsafe extern "C" fn(*const Psn, *const u8) -> i32;

extern "C" {
    fn dlopen(path: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, sym: *const c_char) -> *mut c_void;
}

struct Spi {
    get_front: GetFrontProcess,
    main_cid: MainConnectionId,
    window_owner: GetWindowOwner,
    conn_psn: GetConnectionPsn,
    post: PostEventRecordTo,
}
// SAFETY: the fields are C function pointers into a system framework,
// read-only after one-time resolution; sharing them across threads is sound.
unsafe impl Send for Spi {}
// SAFETY: same as the `Send` impl above — immutable resolved fn pointers.
unsafe impl Sync for Spi {}

static SPI: OnceLock<Option<Spi>> = OnceLock::new();

fn resolve() -> &'static Option<Spi> {
    SPI.get_or_init(|| {
        const RTLD_NOW: c_int = 2;
        let path = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
        // SAFETY: `path` is a valid NUL-terminated C string; dlopen returns
        // null on failure, which is checked.
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
        if handle.is_null() {
            return None;
        }
        // SAFETY: `handle` is a live dlopen handle and each name is a valid
        // C string; dlsym returns null for missing symbols (checked below).
        let sym = |name: &CStr| unsafe { dlsym(handle, name.as_ptr()) };
        let get_front = sym(c"_SLPSGetFrontProcess");
        let main_cid = sym(c"SLSMainConnectionID");
        let window_owner = sym(c"SLSGetWindowOwner");
        let conn_psn = sym(c"SLSGetConnectionPSN");
        let post = sym(c"SLPSPostEventRecordTo");
        if get_front.is_null()
            || main_cid.is_null()
            || window_owner.is_null()
            || conn_psn.is_null()
            || post.is_null()
        {
            return None;
        }
        // SAFETY: each pointer is a non-null symbol from SkyLight with the
        // documented C signature transmuted to the matching fn type.
        unsafe {
            Some(Spi {
                get_front: std::mem::transmute::<*mut c_void, GetFrontProcess>(get_front),
                main_cid: std::mem::transmute::<*mut c_void, MainConnectionId>(main_cid),
                window_owner: std::mem::transmute::<*mut c_void, GetWindowOwner>(window_owner),
                conn_psn: std::mem::transmute::<*mut c_void, GetConnectionPsn>(conn_psn),
                post: std::mem::transmute::<*mut c_void, PostEventRecordTo>(post),
            })
        }
    })
}

pub fn focus_without_raise(window_id: u32) -> bool {
    post_focus_records(window_id, true)
}

/// Give `window_id` key status WITHOUT defocusing the frontmost process.
///
/// The full defocus+focus recipe ([`focus_without_raise`]) activates the
/// target app at the window-server level, and AppKit reacts by ordering every
/// one of that app's windows to the front — a background key delivery must
/// not disturb the user's foreground like that. Skipping the defocus record
/// keeps the user's frontmost app active while the target window still
/// accepts the auth-signed key events posted to its process.
pub fn focus_key_only(window_id: u32) -> bool {
    post_focus_records(window_id, false)
}

fn post_focus_records(window_id: u32, defocus_previous: bool) -> bool {
    let Some(spi) = resolve() else {
        return false;
    };
    // SAFETY: the fns are resolved SkyLight SPIs; `prev`/`target` are
    // correctly sized PSN out-parameters and `buf` is the 248-byte event
    // record the SLPSPostEventRecordTo recipe expects.
    unsafe {
        let cid = (spi.main_cid)();
        let mut owner: u32 = 0;
        if (spi.window_owner)(cid, window_id, &mut owner) != 0 {
            return false;
        }
        let mut target = Psn { high: 0, low: 0 };
        if (spi.conn_psn)(owner, &mut target) != 0 {
            return false;
        }
        let mut buf = [0u8; 0xF8];
        buf[0x04] = 0xF8;
        buf[0x08] = 0x0D;
        buf[0x3C] = (window_id & 0xFF) as u8;
        buf[0x3D] = ((window_id >> 8) & 0xFF) as u8;
        buf[0x3E] = ((window_id >> 16) & 0xFF) as u8;
        buf[0x3F] = ((window_id >> 24) & 0xFF) as u8;
        let defocus = if defocus_previous {
            let mut prev = Psn { high: 0, low: 0 };
            if (spi.get_front)(&mut prev) != 0 {
                return false;
            }
            buf[0x8A] = 0x02; // defocus the previous front
            (spi.post)(&prev, buf.as_ptr())
        } else {
            0
        };
        buf[0x8A] = 0x01; // focus the target window
        let focus = (spi.post)(&target, buf.as_ptr());
        defocus == 0 && focus == 0
    }
}

type EventPostToPid = unsafe extern "C" fn(c_int, *const c_void) -> c_int;
static POST: OnceLock<Option<EventPostToPid>> = OnceLock::new();

/// RTLD_DEFAULT on macOS = `(void*)-2`: dlsym then searches ALL loaded
/// images (so a CoreGraphics private symbol resolves too, not just
/// SkyLight's). We dlopen SkyLight first to make sure it is loaded.
fn rtld_default_sym(name: &CStr) -> *mut c_void {
    let path = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
    // SAFETY: valid C string; result intentionally ignored (just loads it).
    unsafe { dlopen(path.as_ptr(), 2) };
    let rtld_default = (-2_isize) as *mut c_void;
    // SAFETY: RTLD_DEFAULT is the documented global search handle; `name`
    // is a valid C string.
    unsafe { dlsym(rtld_default, name.as_ptr()) }
}

fn post_fn() -> Option<EventPostToPid> {
    *POST.get_or_init(|| {
        let p = rtld_default_sym(c"SLEventPostToPid");
        if p.is_null() {
            None
        } else {
            // SAFETY: non-null SkyLight symbol with the documented C ABI.
            Some(unsafe { std::mem::transmute::<*mut c_void, EventPostToPid>(p) })
        }
    })
}

/// Whether `SLEventPostToPid` resolved (the background mouse path is live).
pub fn mouse_post_available() -> bool {
    post_fn().is_some()
}

type MainConnection = unsafe extern "C" fn() -> u32;
type GetGlobalCursorData = unsafe extern "C" fn(
    u32,        // cid
    *mut u8,    // data
    *mut c_int, // size (in: capacity, out: bytes written)
    *mut c_int, // rowBytes
    *mut f64,   // rect[4]
    *mut f64,   // hotspot[2]
    *mut c_int, // depth
    *mut c_int, // components
) -> c_int;

struct CursorSpi {
    main_cid: MainConnection,
    cursor_data: GetGlobalCursorData,
}
// SAFETY: immutable resolved C function pointers into a system framework.
unsafe impl Send for CursorSpi {}
// SAFETY: same as the `Send` impl above.
unsafe impl Sync for CursorSpi {}

static CURSOR_SPI: OnceLock<Option<CursorSpi>> = OnceLock::new();

fn cursor_spi() -> &'static Option<CursorSpi> {
    CURSOR_SPI.get_or_init(|| {
        let main_cid = rtld_default_sym(c"SLSMainConnectionID");
        let cursor_data = rtld_default_sym(c"SLSGetGlobalCursorData");
        if main_cid.is_null() || cursor_data.is_null() {
            return None;
        }
        // SAFETY: non-null SkyLight symbols with the documented C ABI.
        unsafe {
            Some(CursorSpi {
                main_cid: std::mem::transmute::<*mut c_void, MainConnection>(main_cid),
                cursor_data: std::mem::transmute::<*mut c_void, GetGlobalCursorData>(cursor_data),
            })
        }
    })
}

/// Fingerprint of the current GLOBAL cursor image (FNV-1a over the RGBA bytes
/// SkyLight reports). Two different cursor shapes (arrow vs I-beam, etc.) hash
/// differently; the same shape hashes identically. `None` when the SkyLight
/// cursor SPI is unavailable. Used to tell whether a borrowed-cursor gesture
/// left the pointer stuck in a shape it did not have before.
pub fn cursor_shape_fingerprint() -> Option<u64> {
    let spi = cursor_spi().as_ref()?;
    // SAFETY: `main_cid`/`cursor_data` are the resolved SkyLight SPIs. The
    // buffer is 64 KiB (the cursor image is 32x32x4 = 4 KiB in practice) and
    // `size` carries its capacity in and the byte count out; every other
    // out-parameter points at a correctly sized stack slot.
    unsafe {
        let cid = (spi.main_cid)();
        let mut data = [0u8; 65536];
        let mut size: c_int = data.len() as c_int;
        let mut row_bytes: c_int = 0;
        let mut rect = [0f64; 4];
        let mut hotspot = [0f64; 2];
        let mut depth: c_int = 0;
        let mut components: c_int = 0;
        let err = (spi.cursor_data)(
            cid,
            data.as_mut_ptr(),
            &mut size,
            &mut row_bytes,
            rect.as_mut_ptr(),
            hotspot.as_mut_ptr(),
            &mut depth,
            &mut components,
        );
        if err != 0 || size <= 0 || size as usize > data.len() {
            return None;
        }
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for &byte in &data[..size as usize] {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // Fold in the hotspot so an arrow and an I-beam that happen to share
        // byte histograms still differ (their hotspots differ).
        hash ^= (hotspot[0] as i64 as u64).rotate_left(17);
        hash ^= (hotspot[1] as i64 as u64).rotate_left(31);
        Some(hash)
    }
}

/// Post a CGEvent (raw `CGEventRef`) to `pid` via SkyLight, reaching a
/// backgrounded window's (web) content. Mouse events carry NO auth message
/// (per cua-driver: it diverts them off the IOHID pipeline Chromium reads).
pub fn post_event_to_pid(pid: i32, event: *const c_void) -> bool {
    let Some(f) = post_fn() else {
        return false;
    };
    // SAFETY: `f` is SLEventPostToPid; `event` is a live CGEventRef.
    unsafe { f(pid, event) };
    true
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgPoint {
    x: f64,
    y: f64,
}
type SetWindowLocation = unsafe extern "C" fn(*const c_void, CgPoint);
static SET_WIN_LOC: OnceLock<Option<SetWindowLocation>> = OnceLock::new();

fn set_win_loc_fn() -> Option<SetWindowLocation> {
    *SET_WIN_LOC.get_or_init(|| {
        // CGEventSetWindowLocation lives in CoreGraphics, not SkyLight — so
        // resolve it via the global RTLD_DEFAULT search.
        let p = rtld_default_sym(c"CGEventSetWindowLocation");
        if p.is_null() {
            None
        } else {
            // SAFETY: non-null symbol; documented ABI (CGEventRef, CGPoint).
            Some(unsafe { std::mem::transmute::<*mut c_void, SetWindowLocation>(p) })
        }
    })
}

/// Stamp the window-LOCAL coordinate onto a CGEvent (private SPI). No-op if
/// the symbol is unavailable.
pub fn set_window_location(event: *const c_void, x: f64, y: f64) {
    if let Some(f) = set_win_loc_fn() {
        // SAFETY: `f` is CGEventSetWindowLocation; `event` is a live ref.
        unsafe { f(event, CgPoint { x, y }) };
    }
}

type SetAuthMessage = unsafe extern "C" fn(*const c_void, *mut c_void);
static SET_AUTH: OnceLock<Option<SetAuthMessage>> = OnceLock::new();

fn set_auth_fn() -> Option<SetAuthMessage> {
    *SET_AUTH.get_or_init(|| {
        let p = rtld_default_sym(c"SLEventSetAuthenticationMessage");
        if p.is_null() {
            None
        } else {
            // SAFETY: non-null SkyLight symbol; documented C ABI.
            Some(unsafe { std::mem::transmute::<*mut c_void, SetAuthMessage>(p) })
        }
    })
}

extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn objc_msgSend();
}
type Factory = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, i32, u32) -> *mut c_void;

/// The SkyLight event record lives at a pointer slot inside the opaque
/// CGEvent struct (offset 24/32/16, per cua-driver); return the first
/// non-null. Needed to build the auth message.
fn extract_event_record(event: *const c_void) -> *mut c_void {
    for off in [24usize, 32, 16] {
        // SAFETY: reads a pointer-sized slot inside the live CGEvent struct,
        // a documented (private) layout; non-null is checked by the caller.
        let p = unsafe { *((event as *const u8).add(off) as *const *mut c_void) };
        if !p.is_null() {
            return p;
        }
    }
    std::ptr::null_mut()
}

/// Attach the `SLSEventAuthenticationMessage` envelope macOS 14+ requires
/// for a synthetic KEYBOARD event to be accepted as trusted by web content
/// (Chromium). No-op (unsigned post) if anything fails to resolve.
pub fn attach_auth_message(event: *const c_void, pid: i32) {
    let Some(set_auth) = set_auth_fn() else {
        return;
    };
    let record = extract_event_record(event);
    if record.is_null() {
        return;
    }
    // SAFETY: standard ObjC runtime lookups with valid C strings.
    let class = unsafe { objc_getClass(c"SLSEventAuthenticationMessage".as_ptr()) };
    if class.is_null() {
        return;
    }
    // SAFETY: valid selector string.
    let sel = unsafe { sel_registerName(c"messageWithEventRecord:pid:version:".as_ptr()) };
    // SAFETY: objc_msgSend re-typed to the factory's concrete signature
    // `(Class, SEL, SLSEventRecord*, int32, uint32) -> id`.
    let factory: Factory = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    // SAFETY: invokes the class factory; record/class/sel are valid.
    let msg = unsafe { factory(class, sel, record, pid, 0) };
    if !msg.is_null() {
        // SAFETY: `set_auth` is SLEventSetAuthenticationMessage; args valid.
        unsafe { set_auth(event, msg) };
    }
}
