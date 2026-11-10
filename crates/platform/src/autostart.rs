//! Launch at login.
//!
//! macOS: a user LaunchAgent (`~/Library/LaunchAgents/com.v2rayr.app.plist`) —
//! no admin rights, removable by deleting one file, and it inherits the app's
//! sandbox-free environment. Linux uses an XDG desktop entry; Windows uses
//! the per-user Run registry key.

use std::path::{Path, PathBuf};

use crate::PlatformError;

pub trait Autostart: Send + Sync {
    fn is_enabled(&self) -> bool;
    fn set_enabled(&self, enabled: bool, program: &Path) -> Result<(), PlatformError>;
}

const LABEL: &str = "com.v2rayr.app";

#[derive(Debug, Clone)]
pub struct LaunchAgentAutostart {
    plist_path: PathBuf,
}

impl LaunchAgentAutostart {
    pub fn new(plist_path: impl Into<PathBuf>) -> Self {
        Self {
            plist_path: plist_path.into(),
        }
    }

    pub fn for_home(home: &Path) -> Self {
        Self::new(
            home.join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist")),
        )
    }

    pub fn system() -> Result<Self, PlatformError> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(PlatformError::InvalidState("HOME is not set".to_owned()))?;
        Ok(Self::for_home(&home))
    }

    pub fn path(&self) -> &Path {
        &self.plist_path
    }
}

impl Autostart for LaunchAgentAutostart {
    fn is_enabled(&self) -> bool {
        self.plist_path.is_file()
    }

    fn set_enabled(&self, enabled: bool, program: &Path) -> Result<(), PlatformError> {
        if !enabled {
            match std::fs::remove_file(&self.plist_path) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
        if let Some(parent) = self.plist_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.plist_path, render_plist(program))?;
        Ok(())
    }
}

fn render_plist(program: &Path) -> String {
    let program = xml_escape(&program.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{program}</string>
        <string>--background</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
    )
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabling_writes_a_plist_and_disabling_removes_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let autostart = LaunchAgentAutostart::for_home(dir.path());
        assert!(!autostart.is_enabled());
        autostart
            .set_enabled(
                true,
                Path::new("/Applications/v2rayR.app/Contents/MacOS/v2rayR"),
            )
            .expect("enable");
        assert!(autostart.is_enabled());
        let body = std::fs::read_to_string(autostart.path()).expect("read");
        assert!(body.contains("<string>com.v2rayr.app</string>"));
        assert!(body.contains("/Applications/v2rayR.app/Contents/MacOS/v2rayR"));
        assert!(body.contains("<string>--background</string>"));
        autostart
            .set_enabled(false, Path::new("/x"))
            .expect("disable");
        assert!(!autostart.is_enabled());
        autostart
            .set_enabled(false, Path::new("/x"))
            .expect("idempotent");
    }

    #[test]
    fn desktop_entry_quotes_paths_and_can_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autostart/v2rayr.desktop");
        let backend = DesktopEntryAutostart::new(path.clone());
        backend
            .set_enabled(true, Path::new("/opt/My App/100%$x"))
            .unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains(r#"Exec="/opt/My App/100%%\\$x" --background"#));
        backend.set_enabled(false, Path::new("")).unwrap();
        assert!(!backend.is_enabled());
    }

    #[test]
    fn plist_escapes_xml_metacharacters() {
        let plist = render_plist(Path::new("/Applications/A&B <x>.app/app"));
        assert!(plist.contains("A&amp;B &lt;x&gt;.app"));
    }
}

/// Register the installed executable with the current user's login mechanism.
pub fn set_login_start(enabled: bool, program: &Path) -> Result<(), PlatformError> {
    #[cfg(target_os = "macos")]
    return LaunchAgentAutostart::system()?.set_enabled(enabled, program);
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or_else(|| PlatformError::InvalidState("未找到用户配置目录".into()))?;
        DesktopEntryAutostart::new(config.join("autostart/v2rayr.desktop"))
            .set_enabled(enabled, program)
    }
    #[cfg(windows)]
    return windows::set_enabled(enabled, program);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Err(PlatformError::Unsupported("当前平台不支持登录时启动"))
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug)]
pub struct DesktopEntryAutostart {
    path: PathBuf,
}

#[cfg(any(target_os = "linux", test))]
impl DesktopEntryAutostart {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[cfg(any(target_os = "linux", test))]
impl Autostart for DesktopEntryAutostart {
    fn is_enabled(&self) -> bool {
        self.path.is_file()
    }

    fn set_enabled(&self, enabled: bool, program: &Path) -> Result<(), PlatformError> {
        use std::io::Write;
        if !enabled {
            return match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            };
        }
        let path = program
            .to_str()
            .filter(|_| program.is_absolute())
            .ok_or_else(|| PlatformError::InvalidState("登录项需要有效的绝对路径".into()))?;
        if path.chars().any(char::is_control) {
            return Err(PlatformError::InvalidState("登录项路径包含控制字符".into()));
        }
        // Exec quoting is applied after desktop-entry backslash escaping.
        let exec: String = path
            .chars()
            .flat_map(|ch| match ch {
                '%' => vec!['%', '%'],
                '"' | '`' | '$' => vec!['\\', '\\', ch],
                '\\' => vec!['\\', '\\', '\\', '\\'],
                _ => vec![ch],
            })
            .collect();
        let parent = self
            .path
            .parent()
            .ok_or_else(|| PlatformError::InvalidState("登录项目录无效".into()))?;
        std::fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        write!(
            temp,
            "[Desktop Entry]\nType=Application\nName=v2rayR\nExec=\"{exec}\" --background\nTerminal=false\n"
        )?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path).map_err(|failure| failure.error)?;
        Ok(())
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

    pub(super) fn set_enabled(enabled: bool, program: &Path) -> Result<(), PlatformError> {
        let command = |args: &[&str]| std::process::Command::new("reg.exe").args(args).output();
        let name = "v2rayR";
        let queried = command(&["query", KEY, "/v", name])?;
        if !enabled && !queried.status.success() {
            // Missing value is an idempotent disable; inability to read the
            // containing key is not enough evidence of that.
            if command(&["query", KEY])?.status.success() {
                return Ok(());
            }
            return Err(PlatformError::InvalidState(
                "无法读取 Windows 登录启动项".into(),
            ));
        }
        let output = if enabled {
            let path = program
                .to_str()
                .filter(|_| program.is_absolute())
                .ok_or_else(|| PlatformError::InvalidState("登录项需要有效的绝对路径".into()))?;
            if path.contains('"') || path.chars().any(char::is_control) {
                return Err(PlatformError::InvalidState("Windows 登录项路径无效".into()));
            }
            let value = format!("\"{path}\" --background");
            if value.encode_utf16().count() > 260 {
                return Err(PlatformError::InvalidState("Windows 登录项路径过长".into()));
            }
            command(&["add", KEY, "/v", name, "/t", "REG_SZ", "/d", &value, "/f"])?
        } else {
            command(&["delete", KEY, "/v", name, "/f"])?
        };
        if !output.status.success()
            || command(&["query", KEY, "/v", name])?.status.success() != enabled
        {
            return Err(PlatformError::InvalidState(
                "Windows 登录启动项未通过读回验证".into(),
            ));
        }
        Ok(())
    }
}
