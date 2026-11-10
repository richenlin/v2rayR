//! One elevated Windows bridge per session. The GUI retains a private pipe and
//! checks the PID returned by UAC; the helper checks the pipe server PID.
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::{
    Foundation::{LocalFree, WAIT_TIMEOUT},
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, SECURITY_ATTRIBUTES,
    },
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
            CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, GetExitCodeProcess, GetProcessId,
            WaitForSingleObject,
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

const ROOT: &str = r"C:\Program Files\v2rayR-TUN";
pub const RUNTIME: &str = r"C:\Program Files\v2rayR-TUN\run";
const DLL: &str = r"C:\Program Files\v2rayR-TUN\wintun.dll";
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

pub fn provisioned() -> bool {
    [r"C:\Program Files", ROOT, RUNTIME, HELPER, BRIDGE_CORE, DLL]
        .iter()
        .enumerate()
        .all(|(index, path)| {
            std::fs::symlink_metadata(path).is_ok_and(|metadata| {
                metadata.file_attributes() & 0x400 == 0
                    && if index < 3 {
                        metadata.is_dir()
                    } else {
                        metadata.is_file()
                    }
            })
        })
}

fn ps(script: &str) -> bool {
    Command::new(POWERSHELL)
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
        .output()
        .is_ok_and(|output| output.status.success())
}

pub fn validate_install() -> io::Result<()> {
    if !provisioned() {
        return Err(invalid(
            "TUN installation missing or contains reparse points",
        ));
    }
    // Known-folder default ACL is an OS prerequisite. For the installed
    // subtree require an administrator-owned DACL with no unprivileged writes.
    if !ps(r#"
$ErrorActionPreference = 'Stop'
$root = 'C:\Program Files\v2rayR-TUN'
$admins = @('S-1-5-32-544','S-1-5-18')
$read = [long]([System.Security.AccessControl.FileSystemRights]::ReadAndExecute -bor [System.Security.AccessControl.FileSystemRights]::Synchronize)
foreach ($p in @($root, "$root\run", "$root\v2rayr-helper.exe", "$root\sing-box.exe", "$root\wintun.dll")) {
  $a = Get-Acl -LiteralPath $p
  if ($admins -notcontains $a.GetOwner([System.Security.Principal.SecurityIdentifier]).Value) { exit 1 }
  foreach ($r in $a.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])) {
    if ($r.AccessControlType -eq 'Allow' -and $admins -notcontains $r.IdentityReference.Value -and (([long]$r.FileSystemRights -band (-bnot $read)) -ne 0)) { exit 1 }
  }
}
exit 0
"#)
    {
        return Err(invalid(
            "TUN installation ACL is not administrator-controlled",
        ));
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

pub fn is_admin() -> bool {
    // SAFETY: queries the current process token.
    unsafe { IsUserAnAdmin() != 0 }
}

pub fn open_for_helper(nonce: &str, caller: u32) -> io::Result<File> {
    if !is_admin() || nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("invalid elevated pipe invocation"));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_name(nonce))?;
    let mut pid = 0;
    // SAFETY: file owns the connected pipe and pid points to writable storage.
    if unsafe { GetNamedPipeServerProcessId(file.as_raw_handle(), &mut pid) } == 0 || pid != caller
    {
        return Err(invalid("pipe server is not the requesting app process"));
    }
    Ok(file)
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
        &format!("tun-pipe {nonce} {}", std::process::id()),
        std::path::Path::new(ROOT),
    )
}

/// Installs the protected TUN directory by elevating a staged helper.
///
/// `helper` must be a regular `v2rayr-helper.exe` sitting beside `sing-box.exe`
/// and `wintun.dll`. The elevated process copies that trio into Program Files.
pub async fn elevate_install(helper: &std::path::Path) -> Result<(), PlatformError> {
    let helper = helper.to_path_buf();
    let report = helper.with_file_name("install-error.txt");
    let _ = std::fs::remove_file(&report);
    let helper_for_wait = helper.clone();
    let started = tokio::task::spawn_blocking(move || {
        let directory = helper_for_wait
            .parent()
            .ok_or_else(|| io::Error::other("无法定位 Windows TUN 安装目录"))?;
        shell_execute(&helper_for_wait, "tun-install", directory)
    })
    .await
    .map_err(io::Error::other)?;
    let process = match started {
        Ok(process) => process,
        Err(error) if error.raw_os_error() == Some(1223) => {
            return Err(PlatformError::PermissionDenied(
                "已取消 Windows TUN 管理员授权".into(),
            ));
        }
        Err(error) => {
            return Err(PlatformError::PermissionDenied(format!(
                "无法启动 Windows TUN 安装：{error}"
            )));
        }
    };
    let exit = tokio::task::spawn_blocking(move || wait_exit(&process))
        .await
        .map_err(io::Error::other)?
        .map_err(PlatformError::Io)?;
    if exit == 0 && provisioned() && validate_install().is_ok() {
        return Ok(());
    }
    let detail = std::fs::read_to_string(&report)
        .unwrap_or_else(|_| format!("安装程序退出码 {exit}，没有留下原因"));
    Err(PlatformError::PermissionDenied(format!(
        "安装 Windows TUN 组件失败：{}",
        detail.trim()
    )))
}

fn wait_exit(process: &OwnedHandle) -> io::Result<u32> {
    // SAFETY: the handle is owned and the exit code points at writable storage.
    let waited = unsafe { WaitForSingleObject(process.as_raw_handle(), 120_000) };
    if waited != 0 {
        return Err(io::Error::other("Windows TUN 安装未在两分钟内结束"));
    }
    let mut code = 1;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(code)
}

pub fn bundle_files(
    dir: &std::path::Path,
) -> io::Result<(std::path::PathBuf, std::path::PathBuf, std::path::PathBuf)> {
    let helper = dir.join("v2rayr-helper.exe");
    let core = dir.join("sing-box.exe");
    let dll = dir.join("wintun.dll");
    for path in [&helper, &core, &dll] {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| {
            invalid("Windows TUN 安装目录缺少 v2rayr-helper.exe、sing-box.exe 或 wintun.dll")
        })?;
        if metadata.file_attributes() & 0x400 != 0 || !metadata.is_file() {
            return Err(invalid("Windows TUN 安装文件不能是符号链接或目录联接"));
        }
    }
    Ok((helper, core, dll))
}

/// Copies the staged helper, sing-box and Wintun into the administrator directory.
///
/// Must run elevated. The source is the directory of this process, never a path
/// supplied by the unelevated caller.
pub fn install_protected_bundle() -> io::Result<()> {
    if !is_admin() {
        return Err(invalid("安装 Windows TUN 需要管理员权限"));
    }
    let exe = std::env::current_exe()?;
    let Some(source) = exe.parent() else {
        return Err(invalid("无法定位 Windows TUN 安装源"));
    };
    if source == std::path::Path::new(ROOT) {
        return validate_install();
    }
    let (helper, core, dll) = bundle_files(source)?;
    if std::path::Path::new(ROOT).exists() {
        return Err(invalid(
            "C:\\Program Files\\v2rayR-TUN 已存在但未通过校验，请由管理员检查后删除再重试",
        ));
    }
    let result = (|| {
        std::fs::create_dir(ROOT)?;
        icacls(&[
            ROOT,
            "/inheritance:r",
            "/grant:r",
            "*S-1-5-18:(OI)(CI)F",
            "*S-1-5-32-544:(OI)(CI)F",
            "*S-1-5-32-545:(OI)(CI)RX",
        ])?;
        std::fs::create_dir(RUNTIME)?;
        std::fs::copy(&helper, HELPER)?;
        std::fs::copy(&core, BRIDGE_CORE)?;
        std::fs::copy(&dll, DLL)?;
        icacls(&[ROOT, "/setowner", "*S-1-5-32-544", "/T"])?;
        validate_install()
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(ROOT);
    }
    result
}

fn icacls(args: &[&str]) -> io::Result<()> {
    let output = Command::new(r"C:\Windows\System32\icacls.exe")
        .args(args)
        .env_clear()
        .env("SystemRoot", r"C:\Windows")
        .env("WINDIR", r"C:\Windows")
        .env("PATH", r"C:\Windows\System32")
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = if detail.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout)
        } else {
            detail
        };
        Err(io::Error::other(format!(
            "设置 TUN 目录权限失败：{}",
            detail.trim()
        )))
    }
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
    pub async fn start(request: &TunRequest) -> Result<Self, PlatformError> {
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let mut pipe = server(&nonce)?;
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
        tokio::time::timeout(Duration::from_secs(10), pipe.connect())
            .await
            .map_err(|_| invalid("Windows TUN helper did not connect"))??;
        let mut pid = 0;
        // SAFETY: both handles remain owned for the duration of the checks.
        if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut pid) } == 0
            || pid == 0
            || pid != unsafe { GetProcessId(process.as_raw_handle()) }
        {
            return Err(invalid("pipe peer does not match the elevated TUN helper").into());
        }
        let mut frame = serde_json::to_vec(request).map_err(io::Error::other)?;
        frame.push(b'\n');
        pipe.write_all(&frame).await?;
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
        Ok(Self {
            pipe: Some(reader.into_inner()),
            process,
        })
    }

    pub fn exited(&self) -> bool {
        // SAFETY: this is a nonblocking query on an owned process handle.
        unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) != WAIT_TIMEOUT }
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

pub struct Job(OwnedHandle);
impl Job {
    pub fn new() -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: buffer matches the requested Windows info class.
        if unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }
    pub fn assign(&self, child: &Child) -> io::Result<()> {
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
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
    use super::bundle_files;

    #[test]
    fn bundle_requires_the_three_regular_files() {
        let dir = tempfile::tempdir().expect("temp");
        std::fs::write(dir.path().join("v2rayr-helper.exe"), b"helper").unwrap();
        std::fs::write(dir.path().join("sing-box.exe"), b"core").unwrap();
        assert!(bundle_files(dir.path()).is_err());
        std::fs::write(dir.path().join("wintun.dll"), b"dll").unwrap();
        let (helper, core, dll) = bundle_files(dir.path()).expect("bundle");
        assert_eq!(helper.file_name().unwrap(), "v2rayr-helper.exe");
        assert_eq!(core.file_name().unwrap(), "sing-box.exe");
        assert_eq!(dll.file_name().unwrap(), "wintun.dll");
    }
}
