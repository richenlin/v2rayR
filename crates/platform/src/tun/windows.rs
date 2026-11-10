//! One elevated Windows bridge per session. The GUI retains a private pipe and
//! checks the PID returned by UAC; the helper checks the pipe server PID.
use std::ffi::c_void;
use std::io;

mod recovery;
pub use recovery::{capture_network, recovery_pending, restore_network};
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows_sys::Win32::{
    Foundation::{LocalFree, WAIT_OBJECT_0},
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, CreateWellKnownSid,
        SECURITY_ATTRIBUTES, SECURITY_MAX_SID_SIZE, SetTokenInformation, TOKEN_ADJUST_DEFAULT,
        TOKEN_OWNER, TokenOwner, WinBuiltinAdministratorsSid,
    },
    Storage::FileSystem::SECURITY_IDENTIFICATION,
    System::{
        Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize},
        Console::{AllocConsole, CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent, GetConsoleCP},
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        },
        Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
        Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, GetCurrentProcess, GetExitCodeProcess,
            GetProcessId, OpenProcessToken, WaitForSingleObject,
        },
    },
    UI::{
        Shell::{
            IsUserAnAdmin, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
            ShellExecuteExW,
        },
        WindowsAndMessaging::SW_HIDE,
    },
};

use super::{BRIDGE_CORE, HELPER, TunRequest};
use crate::PlatformError;

const ROOT: &str = r"C:\Program Files\v2rayR\windows-tun";
pub const RUNTIME: &str = r"C:\Program Files\v2rayR\windows-tun\run";
const MANIFEST: &str = r"C:\Program Files\v2rayR\windows-tun\bundle-v2.sha256";
const DLL: &str = r"C:\Program Files\v2rayR\windows-tun\wintun.dll";
const POWERSHELL: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
fn pipe_name(nonce: &str) -> String {
    format!(r"\\.\pipe\v2rayr-tun-{nonce}")
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

fn regular(path: &str, directory: bool) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_attributes() & 0x400 == 0
            && if directory {
                metadata.is_dir()
            } else {
                metadata.is_file()
            }
    })
}

pub fn provisioned() -> bool {
    [r"C:\Program Files", r"C:\Program Files\v2rayR", ROOT]
        .iter()
        .all(|path| regular(path, true))
        && [HELPER, BRIDGE_CORE, DLL, MANIFEST]
            .iter()
            .all(|path| regular(path, false))
}

fn ps(script: &str) -> bool {
    let child = Command::new(POWERSHELL)
        .env_clear()
        .env("SystemRoot", r"C:\Windows")
        .env("WINDIR", r"C:\Windows")
        .env("PATH", r"C:\Windows\System32")
        .env(
            "PSModulePath",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\Modules",
        )
        .current_dir(r"C:\Windows\System32")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return false };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

pub fn validate_install() -> io::Result<()> {
    if !provisioned() {
        return Err(invalid(
            "请在默认 Program Files 目录安装或修复 Windows TUN 组件",
        ));
    }
    if !ps(r#"
$ErrorActionPreference = 'Stop'
$app = 'C:\Program Files\v2rayR'
$root = "$app\windows-tun"
$trusted = @('S-1-5-32-544','S-1-5-18','S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
$read = [long]([System.Security.AccessControl.FileSystemRights]::ReadAndExecute -bor [System.Security.AccessControl.FileSystemRights]::Synchronize)
$paths = @('C:\Program Files', $app, $root, "$root\v2rayr-helper.exe", "$root\sing-box.exe", "$root\wintun.dll", "$root\bundle-v2.sha256")
if (Test-Path -LiteralPath "$root\run") { $paths += "$root\run" }
foreach ($p in $paths) {
  if ((Get-Item -LiteralPath $p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { exit 1 }
  $a = Get-Acl -LiteralPath $p
  $raw = [System.Security.AccessControl.RawSecurityDescriptor]::new($a.GetSecurityDescriptorBinaryForm(), 0)
  if ($null -eq $raw.DiscretionaryAcl -or $trusted -notcontains $raw.Owner.Value) { exit 1 }
  foreach ($r in $a.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])) {
    if (($r.PropagationFlags -band [System.Security.AccessControl.PropagationFlags]::InheritOnly) -ne 0) { continue }
    if ($r.AccessControlType -eq 'Allow' -and $trusted -notcontains $r.IdentityReference.Value -and (([long]$r.FileSystemRights -band (-bnot $read)) -ne 0)) { exit 1 }
  }
}
# The dedicated executable directory also controls the DLL search path.
$allowed = @('v2rayr-helper.exe','sing-box.exe','wintun.dll','Wintun-LICENSE.txt','bundle-v2.sha256','run')
if (Get-ChildItem -LiteralPath $root -Force | Where-Object { $_.Name -notin $allowed }) { exit 1 }
$files = @('v2rayr-helper.exe','sing-box.exe','wintun.dll')
$hashes = @(Get-Content -LiteralPath "$root\bundle-v2.sha256")
if ($hashes.Count -ne ($files.Count + 1)) { exit 1 }
for ($i = 0; $i -lt $files.Count; $i++) {
  $file = $files[$i]
  $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath "$root\$file").Hash
  if ($hashes[$i + 1] -ine "$hash  $file") { exit 1 }
}
exit 0
"#)
    {
        return Err(invalid(
            "Windows TUN 组件的权限或完整性检查失败，请使用受信任的安装包修复",
        ));
    }
    use std::io::BufRead;
    let manifest = std::fs::File::open(MANIFEST)?;
    let mut version = String::new();
    io::BufReader::new(io::Read::take(manifest, 256)).read_line(&mut version)?;
    if version.trim_end() != concat!("v2rayr-helper ", env!("CARGO_PKG_VERSION")) {
        return Err(invalid("Windows TUN 组件版本不匹配，请升级或修复安装"));
    }
    Ok(())
}

pub fn lock_runtime() -> io::Result<crate::InstanceGuard> {
    if !is_admin()
        || std::fs::canonicalize(std::env::current_exe()?)? != std::fs::canonicalize(HELPER)?
    {
        return Err(invalid(
            "Windows TUN requires the installed elevated helper",
        ));
    }
    validate_install()?;
    set_runtime_owner()?;
    let lock = std::path::Path::new(RUNTIME).join("v2rayr-tun-bridge.lock");
    match std::fs::symlink_metadata(&lock) {
        Ok(metadata) if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 => {
            return Err(invalid("Windows TUN lock is not a regular file"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    match crate::acquire_single_instance(&lock).map_err(io::Error::other)? {
        crate::InstanceOutcome::Primary(guard) => Ok(guard),
        crate::InstanceOutcome::AlreadyRunning(_) => {
            Err(invalid("Windows TUN bridge already in use"))
        }
    }
}

fn set_runtime_owner() -> io::Result<()> {
    // The medium-integrity user also has the user's SID. If that SID owns the
    // files created by the helper, it can change their DACL despite inheritance.
    let mut sid = [0u32; SECURITY_MAX_SID_SIZE as usize / size_of::<u32>()];
    let mut size = SECURITY_MAX_SID_SIZE;
    let sid = sid.as_mut_ptr().cast();
    let mut token = null_mut();
    // SAFETY: aligned storage holds the full well-known SID and token out handle.
    if unsafe { CreateWellKnownSid(WinBuiltinAdministratorsSid, null_mut(), sid, &mut size) } == 0
        || unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_DEFAULT, &mut token) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let owner = TOKEN_OWNER { Owner: sid };
    if unsafe {
        SetTokenInformation(
            token.as_raw_handle(),
            TokenOwner,
            (&owner as *const TOKEN_OWNER).cast(),
            size_of::<TOKEN_OWNER>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn interface_ready() -> bool {
    ps(r#"
$ErrorActionPreference = 'Stop'
foreach ($pair in @(@('172.30.254.1','198.51.100.20'), @('fd5b:67f3:2a9c::1','2001:db8::20'))) {
  $i = Get-NetIPAddress -InterfaceAlias 'v2rayr-tun0' -IPAddress $pair[0]
  $r = Find-NetRoute -RemoteIPAddress $pair[1] | Where-Object { $_.CimClass.CimClassName -eq 'MSFT_NetRoute' }
  if (!$r -or $r.InterfaceIndex -ne $i.InterfaceIndex) { exit 1 }
}
exit 0
"#)
}

pub fn network_cleared() -> bool {
    ps(r#"
$ErrorActionPreference = 'Stop'
$i = @(Get-NetIPAddress | Where-Object { $_.IPAddress -in @('172.30.254.1','fd5b:67f3:2a9c::1') })
if ($i.Count) { exit 1 }
$a = @(Get-NetIPInterface | Where-Object { $_.InterfaceAlias -eq 'v2rayr-tun0' })
if ($a.Count) {
  $r = @(Get-NetRoute | Where-Object { $_.InterfaceIndex -in $a.InterfaceIndex })
  if ($r.Count) { exit 1 }
}
exit 0
"#)
}

fn clear_owned_adapter() -> bool {
    // sing-tun derives the adapter GUID from MD5("wintun" + interface name).
    // An unrelated device reusing the alias must never be modified.
    ps(r#"
$ErrorActionPreference = 'Stop'
$a = @(Get-NetAdapter -IncludeHidden | Where-Object {
  $_.Name -eq 'v2rayr-tun0' -and [guid]$_.InterfaceGuid -eq [guid]'86384023-0cb4-0b76-aec1-6829986218d7'
})
if ($a.Count -ne 1) { exit 1 }
$idx = $a[0].ifIndex
Get-NetRoute -PolicyStore ActiveStore | Where-Object { $_.InterfaceIndex -eq $idx } |
  Remove-NetRoute -Confirm:$false
Get-NetIPAddress -PolicyStore ActiveStore | Where-Object {
  $_.InterfaceIndex -eq $idx -and $_.IPAddress -in @('172.30.254.1','fd5b:67f3:2a9c::1')
} | Remove-NetIPAddress -Confirm:$false
Set-DnsClientServerAddress -InterfaceIndex $idx -ResetServerAddresses
exit 0
"#) && network_cleared()
}

pub fn is_admin() -> bool {
    // SAFETY: queries the current process token.
    unsafe { IsUserAnAdmin() != 0 }
}

pub fn open_for_helper(nonce: &str, caller: u32) -> io::Result<NamedPipeClient> {
    if !is_admin() || nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("invalid elevated pipe invocation"));
    }
    let pipe = open_client(nonce)?;
    let mut pid = 0;
    // SAFETY: pipe owns the connected handle and pid points to writable storage.
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) } == 0 || pid != caller
    {
        return Err(invalid("pipe server is not the requesting app process"));
    }
    Ok(pipe)
}

fn open_client(nonce: &str) -> io::Result<NamedPipeClient> {
    // Tokio opens an overlapped handle. Limit the unelevated server to token
    // identification even though the client is elevated.
    ClientOptions::new()
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(pipe_name(nonce))
}

fn server(nonce: &str) -> io::Result<NamedPipeServer> {
    // Limit access to the creating token's owner and LocalSystem. A different
    // admin account used in the UAC prompt is deliberately not supported.
    let descriptor = wide("D:P(A;;GA;;;OW)(A;;GA;;;SY)");
    let mut sd: *mut c_void = null_mut();
    // SAFETY: descriptor is null-terminated; Windows allocates sd on success.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor.as_ptr(),
            1,
            &mut sd,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    // SAFETY: attributes and its descriptor live until CreateNamedPipe returns.
    let pipe = unsafe {
        ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .max_instances(1)
            .create_with_security_attributes_raw(
                pipe_name(nonce),
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            )
    };
    unsafe { LocalFree(sd) };
    pipe
}

fn elevate(nonce: &str) -> io::Result<OwnedHandle> {
    validate_install()?;
    shell_execute(
        std::path::Path::new(HELPER),
        &format!("tun-pipe-v2 {nonce} {}", std::process::id()),
        std::path::Path::new(ROOT),
    )
}

pub async fn recover_pending() -> Result<(), PlatformError> {
    if !recovery_pending()? {
        return Ok(());
    }
    tokio::task::spawn_blocking(|| {
        validate_install()?;
        let process = shell_execute(
            std::path::Path::new(HELPER),
            "tun-recover-v2",
            std::path::Path::new(ROOT),
        )?;
        if unsafe { WaitForSingleObject(process.as_raw_handle(), 15_000) } != WAIT_OBJECT_0 {
            return Err(io::Error::other("Windows TUN 恢复尚未完成"));
        }
        let mut exit = 1;
        if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut exit) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if exit != 0 || recovery_pending()? {
            return Err(io::Error::other("Windows TUN 网络恢复失败，请重试恢复"));
        }
        Ok(())
    })
    .await
    .map_err(io::Error::other)??;
    Ok(())
}

fn shell_execute(
    exe: &std::path::Path,
    parameters: &str,
    directory: &std::path::Path,
) -> io::Result<OwnedHandle> {
    use std::os::windows::ffi::OsStrExt;
    let wide_os = |value: &std::ffi::OsStr| value.encode_wide().chain([0]).collect::<Vec<_>>();
    let verb = wide("runas");
    let exe = wide_os(exe.as_os_str());
    let args = wide(parameters);
    let directory = wide_os(directory.as_os_str());
    // SAFETY: initialized for this blocking worker only; all strings and the
    // structure are live until the synchronous ShellExecuteEx call returns.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of_val(&info) as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = exe.as_ptr();
    info.lpParameters = args.as_ptr();
    info.lpDirectory = directory.as_ptr();
    info.nShow = SW_HIDE;
    let init = unsafe {
        CoInitializeEx(
            null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    if init < 0 {
        return Err(io::Error::other("cannot initialize Windows UAC launcher"));
    }
    let result = unsafe { ShellExecuteExW(&mut info) };
    let failure = io::Error::last_os_error();
    unsafe { CoUninitialize() };
    if result == 0 || info.hProcess.is_null() {
        return Err(failure);
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(info.hProcess) })
}

#[derive(Debug)]
pub struct Lease {
    pipe: Option<NamedPipeServer>,
    process: OwnedHandle,
}

impl Lease {
    pub async fn launch() -> Result<Self, PlatformError> {
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let pipe = server(&nonce)?;
        let process = tokio::task::spawn_blocking(move || elevate(&nonce))
            .await
            .map_err(io::Error::other)?
            .map_err(|error| {
                PlatformError::PermissionDenied(if error.raw_os_error() == Some(1223) {
                    "已取消 Windows TUN 管理员授权".into()
                } else {
                    format!("无法启动 Windows TUN helper：{error}")
                })
            })?;
        Ok(Self {
            pipe: Some(pipe),
            process,
        })
    }

    pub async fn wait_ready(&mut self, request: &TunRequest) -> Result<(), PlatformError> {
        let pipe = self.pipe.as_mut().expect("lease has not been stopped");
        tokio::time::timeout(Duration::from_secs(10), pipe.connect())
            .await
            .map_err(|_| invalid("Windows TUN helper did not connect"))??;
        let mut pid = 0;
        // SAFETY: both handles remain owned for the duration of the checks.
        if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut pid) } == 0
            || pid == 0
            || pid != unsafe { GetProcessId(self.process.as_raw_handle()) }
        {
            return Err(invalid("pipe peer does not match the elevated TUN helper").into());
        }
        let mut frame = serde_json::to_vec(request).map_err(io::Error::other)?;
        frame.push(b'\n');
        tokio::time::timeout(Duration::from_secs(5), pipe.write_all(&frame))
            .await
            .map_err(|_| invalid("Windows TUN request timed out"))??;
        let mut reader = BufReader::new(pipe);
        let mut ready = String::new();
        use tokio::io::AsyncReadExt;
        tokio::time::timeout(
            Duration::from_secs(30),
            (&mut reader).take(2048).read_line(&mut ready),
        )
        .await
        .map_err(|_| invalid("Windows TUN helper readiness timed out"))??;
        if ready != "ready\n" {
            let detail = ready.trim();
            let detail = if detail.is_empty() {
                "Windows TUN helper 没有确认网卡和路由"
            } else {
                detail
            };
            return Err(invalid(detail).into());
        }
        Ok(())
    }

    pub fn exited(&self) -> bool {
        // SAFETY: this is a nonblocking query on an owned process handle.
        unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) == WAIT_OBJECT_0 }
    }

    pub async fn stop(&mut self) -> Result<(), PlatformError> {
        drop(self.pipe.take());
        let deadline = Instant::now() + Duration::from_secs(12);
        while !self.exited() {
            if Instant::now() >= deadline {
                return Err(PlatformError::InvalidState(
                    "Windows TUN helper 尚未退出".into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
        recover_pending().await?;
        if !tokio::task::spawn_blocking(network_cleared)
            .await
            .map_err(io::Error::other)?
        {
            return Err(PlatformError::InvalidState(
                "Windows TUN 路由尚未恢复".into(),
            ));
        }
        Ok(())
    }
}

pub fn guard_core_lifetime() -> io::Result<()> {
    // Join the job before spawning any core. Descendants inherit membership
    // from the helper even if the helper is killed during process creation.
    // The helper holds the only job handle for its entire process lifetime.
    static JOB: std::sync::OnceLock<OwnedHandle> = std::sync::OnceLock::new();
    let handle = unsafe { CreateJobObjectW(null(), null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: the information buffer matches the Windows job info class.
    if unsafe {
        SetInformationJobObject(
            handle.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    } == 0
        || unsafe { AssignProcessToJobObject(handle.as_raw_handle(), GetCurrentProcess()) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    JOB.set(handle)
        .map_err(|_| io::Error::other("TUN helper job is already initialized"))
}

pub fn prepare_core(command: &mut Command) -> io::Result<()> {
    // CTRL_BREAK targets the child's process group in the helper's console.
    // The helper itself must not share the child's process group.
    if unsafe { GetConsoleCP() == 0 && AllocConsole() == 0 } {
        return Err(io::Error::last_os_error());
    }
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    Ok(())
}

pub fn request_core_stop(child: &Child) {
    // SAFETY: caller owns the child started in its own process group.
    unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pipe_replies_while_lease_read_is_pending() {
        use tokio::io::AsyncReadExt;
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let mut server = server(&nonce).unwrap();
        let client = open_client(&nonce).unwrap();
        server.connect().await.unwrap();
        let (mut read, mut write) = tokio::io::split(client);
        let mut buffer = [0];
        let mut eof = std::pin::pin!(read.read(&mut buffer));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut eof)
                .await
                .is_err()
        );
        write.write_all(b"ready\n").await.unwrap();
        let mut ready = [0; 6];
        tokio::time::timeout(Duration::from_secs(2), server.read_exact(&mut ready))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&ready, b"ready\n");
        drop(server);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), eof)
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }
}
