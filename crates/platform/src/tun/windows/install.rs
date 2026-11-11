use std::ffi::c_void;
use std::io::{self, Read};
use std::path::Path;
use std::ptr::{addr_of, null_mut};

use sha2::{Digest, Sha256};
use windows_sys::Win32::{
    Foundation::{GENERIC_EXECUTE, GENERIC_READ, LocalFree},
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER,
        Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT},
        DACL_SECURITY_INFORMATION, GetAce, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION, PSID,
    },
    Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ},
    System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
};

use super::{BRIDGE_CORE, DLL, HELPER, MANIFEST, ROOT, RUNTIME, invalid, regular, wide};

const DIRECTORIES: [&str; 3] = [r"C:\Program Files", r"C:\Program Files\v2rayR", ROOT];
const PAYLOAD: [&str; 3] = [HELPER, BRIDGE_CORE, DLL];
const READ_EXECUTE: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE | GENERIC_READ | GENERIC_EXECUTE;

struct Allocation(*mut c_void);

impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: the address was returned by a Win32 LocalAlloc-based API.
        unsafe { LocalFree(self.0) };
    }
}

fn trusted(sid: PSID) -> io::Result<bool> {
    let mut text = null_mut();
    // SAFETY: the SID is owned by the descriptor and text is an out parameter.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _text = Allocation(text.cast());
    let text = unsafe { std::slice::from_raw_parts(text, wide_len(text)) };
    Ok([
        "S-1-5-18",                                                       // LocalSystem
        "S-1-5-32-544",                                                   // Administrators
        "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464", // TrustedInstaller
    ]
    .iter()
    .any(|sid| text.iter().copied().eq(sid.encode_utf16())))
}

unsafe fn wide_len(text: *const u16) -> usize {
    let mut length = 0;
    // SAFETY: caller provides a readable NUL-terminated string.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    length
}

fn check_acl(path: &str) -> io::Result<()> {
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut descriptor = null_mut();
    // SAFETY: path is NUL-terminated and the out pointers have the expected types.
    let result = unsafe {
        GetNamedSecurityInfoW(
            wide(path).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if result != 0 {
        return Err(invalid(&format!(
            "Cannot read ACL for {path}: {}",
            io::Error::from_raw_os_error(result as i32)
        )));
    }
    let _descriptor = Allocation(descriptor);
    if owner.is_null() || !trusted(owner)? || dacl.is_null() {
        return Err(invalid(&format!("Untrusted owner or missing DACL: {path}")));
    }
    // SAFETY: the descriptor keeps the ACL and all returned ACEs alive.
    for index in 0..u32::from(unsafe { (*dacl).AceCount }) {
        let mut ace = null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        match u32::from(header.AceType) {
            ACCESS_DENIED_ACE_TYPE => {}
            ACCESS_ALLOWED_ACE_TYPE => {
                let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
                if unsafe { (*ace).Mask } & !READ_EXECUTE != 0
                    && !trusted(unsafe { addr_of!((*ace).SidStart).cast_mut().cast() })?
                {
                    return Err(invalid(&format!(
                        "Untrusted write permission in ACE {index}: {path}"
                    )));
                }
            }
            kind => {
                return Err(invalid(&format!(
                    "Unsupported DACL ACE type {kind}: {path}"
                )));
            }
        }
    }
    Ok(())
}

pub fn validate_install() -> io::Result<()> {
    for path in DIRECTORIES {
        if !regular(path, true) {
            return Err(invalid(&format!(
                "Missing or unsafe TUN install directory: {path}"
            )));
        }
        check_acl(path)?;
    }
    for path in PAYLOAD.into_iter().chain([MANIFEST]) {
        if !regular(path, false) {
            return Err(invalid(&format!("Missing or unsafe TUN component: {path}")));
        }
        check_acl(path)?;
    }
    match std::fs::symlink_metadata(RUNTIME) {
        Ok(_) if regular(RUNTIME, true) => check_acl(RUNTIME)?,
        Ok(_) => return Err(invalid("Unsafe Windows TUN runtime directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    // The elevated executables search for DLLs in this directory.
    for entry in std::fs::read_dir(ROOT)? {
        let name = entry?.file_name();
        if ![
            "v2rayr-helper.exe",
            "sing-box.exe",
            "wintun.dll",
            "Wintun-LICENSE.txt",
            "bundle-v2.sha256",
            "run",
        ]
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
        {
            return Err(invalid(&format!(
                "Unexpected TUN install entry: {}",
                name.to_string_lossy()
            )));
        }
    }
    let mut manifest = String::new();
    std::fs::File::open(MANIFEST)?
        .take(1024)
        .read_to_string(&mut manifest)?;
    let mut lines = manifest.lines();
    if lines.next() != Some(concat!("v2rayr-helper ", env!("CARGO_PKG_VERSION"))) {
        return Err(invalid("Windows TUN 组件版本不匹配，请升级或修复安装"));
    }
    for path in PAYLOAD {
        let name = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .expect("fixed component name");
        let mut file = std::fs::File::open(path)?;
        let mut hash = Sha256::new();
        io::copy(&mut file, &mut hash)?;
        let digest = format!("{:x}", hash.finalize());
        let line = lines.next().unwrap_or_default();
        let matches = line.split_once(' ').is_some_and(|(checksum, filename)| {
            // GNU checksum records put a space (text) or an asterisk (binary)
            // before the filename. The file digest is always computed on raw bytes.
            filename.strip_prefix([' ', '*']).is_some_and(|filename| {
                filename.eq_ignore_ascii_case(name) && checksum.eq_ignore_ascii_case(&digest)
            })
        });
        if !matches {
            return Err(invalid(&format!(
                "Windows TUN 组件摘要不匹配: {name}; actual {digest}; manifest {line:?}"
            )));
        }
    }
    if lines.next().is_some() {
        return Err(invalid("Unexpected Windows TUN manifest entries"));
    }
    Ok(())
}
