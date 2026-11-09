//! Launch at login.
//!
//! macOS: a user LaunchAgent (`~/Library/LaunchAgents/com.v2rayr.app.plist`) —
//! no admin rights, removable by deleting one file, and it inherits the app's
//! sandbox-free environment. Linux/Windows return `Unsupported` rather than
//! pretending to work.

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
    fn plist_escapes_xml_metacharacters() {
        let plist = render_plist(Path::new("/Applications/A&B <x>.app/app"));
        assert!(plist.contains("A&amp;B &lt;x&gt;.app"));
    }
}
