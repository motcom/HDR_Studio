use glam::DMat4;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::Path,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct RawNode {
    path: *const c_char,
    parent: i32,
    kind: u32,
    world: [f64; 16],
    positions: *const f32,
    position_count: u64,
    indices: *const i32,
    index_count: u64,
    face_counts: *const i32,
    face_count: u64,
    corner_normals: *const f32,
    normal_count: u64,
    camera_core: [f64; 16],
    film_back: [f64; 9],
    visible: u32,
}

pub struct Node {
    pub path: String,
    pub parent: i32,
    pub kind: u32,
    pub world: DMat4,
    pub positions: Vec<f32>,
    pub indices: Vec<i32>,
    pub counts: Vec<i32>,
    pub normals: Vec<f32>,
    pub camera_core: [f64; 16],
    pub film_back: [f64; 9],
    pub visible: bool,
}

type Open = unsafe extern "C" fn(*const c_char, f64, *mut c_char, u64) -> *mut c_void;
type Close = unsafe extern "C" fn(*mut c_void);
type Count = unsafe extern "C" fn(*const c_void) -> u64;
type Get = unsafe extern "C" fn(*const c_void, u64, *mut RawNode) -> i32;

// The library outlives this guard. C++ owns every borrowed buffer until close.
struct Handle {
    ptr: *mut c_void,
    close: Close,
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { (self.close)(self.ptr) }
    }
}

unsafe fn copy_buffer<T: Copy>(ptr: *const T, len: u64) -> Result<Vec<T>, String> {
    let len = usize::try_from(len).map_err(|_| "Buffer length overflow")?;
    if len == 0 {
        return Ok(Vec::new());
    }
    if ptr.is_null() || len > isize::MAX as usize / std::mem::size_of::<T>() {
        return Err("Invalid buffer returned by ABC DLL".into());
    }
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec())
}

pub fn load(path: &Path, seconds: f64) -> Result<Vec<Node>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = exe.parent().ok_or("Missing executable directory")?;
    let mut dll = directory.join("hdr_abc.dll");
    // cargo test binaries are under target/<profile>/deps.
    if !dll.exists() && directory.file_name().is_some_and(|n| n == "deps") {
        dll = directory.parent().unwrap().join("hdr_abc.dll");
    }
    let path = CString::new(path.to_str().ok_or("Path must be valid UTF-8")?)
        .map_err(|_| "Path contains a NUL byte")?;
    unsafe {
        let library = libloading::Library::new(&dll).map_err(|e| {
            format!(
                "Cannot load {}: {e}. Run scripts/build-native.ps1 first.",
                dll.display()
            )
        })?;
        let version = library
            .get::<unsafe extern "C" fn() -> u32>(b"abc_abi_version\0")
            .map_err(|e| e.to_string())?;
        if version() != 1 {
            return Err("ABC DLL ABI version mismatch".into());
        }
        let open = *library
            .get::<Open>(b"abc_open\0")
            .map_err(|e| e.to_string())?;
        let close = *library
            .get::<Close>(b"abc_close\0")
            .map_err(|e| e.to_string())?;
        let count = *library
            .get::<Count>(b"abc_node_count\0")
            .map_err(|e| e.to_string())?;
        let get = *library
            .get::<Get>(b"abc_get_node\0")
            .map_err(|e| e.to_string())?;
        let mut error = [0 as c_char; 4096];
        let ptr = open(
            path.as_ptr(),
            seconds,
            error.as_mut_ptr(),
            error.len() as u64,
        );
        if ptr.is_null() {
            return Err(CStr::from_ptr(error.as_ptr())
                .to_string_lossy()
                .into_owned());
        }
        let handle = Handle { ptr, close };
        let mut nodes = Vec::new();
        for i in 0..count(handle.ptr) {
            let mut raw = std::mem::MaybeUninit::<RawNode>::uninit();
            if get(handle.ptr, i, raw.as_mut_ptr()) != 1 {
                return Err("Cannot read ABC node".into());
            }
            let raw = raw.assume_init();
            if raw.path.is_null() {
                return Err("Null ABC node path".into());
            }
            nodes.push(Node {
                path: CStr::from_ptr(raw.path).to_string_lossy().into_owned(),
                parent: raw.parent,
                kind: raw.kind,
                world: DMat4::from_cols_array(&raw.world),
                positions: copy_buffer(
                    raw.positions,
                    raw.position_count
                        .checked_mul(3)
                        .ok_or("Position count overflow")?,
                )?,
                indices: copy_buffer(raw.indices, raw.index_count)?,
                counts: copy_buffer(raw.face_counts, raw.face_count)?,
                normals: copy_buffer(
                    raw.corner_normals,
                    raw.normal_count
                        .checked_mul(3)
                        .ok_or("Normal count overflow")?,
                )?,
                camera_core: raw.camera_core,
                film_back: raw.film_back,
                visible: raw.visible != 0,
            });
        }
        Ok(nodes)
    }
}
