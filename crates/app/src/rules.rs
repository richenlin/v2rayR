//! Custom routing rules.
//!
//! Rules are stored in SQLite and read when a session plan is built, so an edit
//! takes effect on the next connect. The UI says so instead of pretending the
//! running session changed.

use v2rayr_domain::{ErrorCode, RuleApplication};
use v2rayr_ipc::Problem;
use v2rayr_ipc::RuleView;

use crate::App;
use crate::error;

impl App {
    pub fn rules(&self) -> Result<Vec<RuleView>, Problem> {
        let rows = self.db().list_rules().map_err(error::from_store)?;
        Ok(rows.into_iter().map(rule_view).collect())
    }

    /// Prepare both halves of the macOS provider handshake from the same
    /// ordered database snapshot. Installation and session activation are
    /// deliberately separate from preparing these transient configurations.
    #[cfg(target_os = "macos")]
    pub fn prepare_macos_application_ingress(
        &self,
        mut plan: v2rayr_config::SessionPlan,
        port: u16,
        kernel: &std::path::Path,
    ) -> Result<(v2rayr_config::EmittedConfig, v2rayr_config::application_ingress::ProviderConfig), Problem> {
        let rows = self.db().enabled_rules().map_err(error::from_store)?;
        let scopes: Vec<_> = rows.iter().map(|row| row.applications.clone()).collect();
        plan.rules = rows.into_iter().map(into_config_rule).collect();
        let kernel = kernel.canonicalize().map_err(error::internal)?;
        let kernel = kernel.to_str().ok_or_else(|| Problem::config("内核路径无法读取"))?;
        v2rayr_config::application_ingress::emit(
            &plan, &scopes, port, crate::session::new_secret(), kernel.to_owned(),
        ).map_err(error::from_config)
    }

    pub fn add_rule(
        &self,
        kind: &str,
        value: &str,
        action: &str,
    ) -> Result<Vec<RuleView>, Problem> {
        self.add_rule_for_applications(kind, value, action, &[])
    }

    pub fn add_rule_for_applications(
        &self,
        kind: &str,
        value: &str,
        action: &str,
        applications: &[RuleApplication],
    ) -> Result<Vec<RuleView>, Problem> {
        let (kind, value, action, applications) = rule_fields(kind, value, action, applications)?;
        self.db()
            .add_rule_for_applications(kind, &value, action, &applications)
            .map_err(error::from_store)?;
        self.logs
            .info("rules", format!("新增规则 {} {}", kind.as_str(), value));
        self.rules()
    }

    pub fn update_rule(
        &self,
        id: uuid::Uuid,
        kind: &str,
        value: &str,
        action: &str,
        applications: &[RuleApplication],
    ) -> Result<Vec<RuleView>, Problem> {
        let (kind, value, action, applications) = rule_fields(kind, value, action, applications)?;
        if !self.db().update_rule(id, kind, &value, action, &applications).map_err(error::from_store)? {
            return Err(Problem::new(ErrorCode::NotFound.as_str(), false).with_detail("规则不存在"));
        }
        self.rules()
    }

    pub fn set_rule_enabled(
        &self,
        id: uuid::Uuid,
        enabled: bool,
    ) -> Result<Vec<RuleView>, Problem> {
        if !self
            .db()
            .list_rules()
            .map_err(error::from_store)?
            .iter()
            .any(|rule| rule.id == id)
        {
            return Err(Problem::new(ErrorCode::NotFound.as_str(), false)
                .with_detail("规则不存在".to_owned()));
        }
        self.db()
            .set_rule_enabled(id, enabled)
            .map_err(error::from_store)?;
        self.rules()
    }

    pub fn remove_rule(&self, id: uuid::Uuid) -> Result<Vec<RuleView>, Problem> {
        let removed = self.db().delete_rule(id).map_err(error::from_store)?;
        if !removed {
            return Err(Problem::new(ErrorCode::NotFound.as_str(), false)
                .with_detail("规则不存在".to_owned()));
        }
        self.rules()
    }
}

fn rule_fields(
    kind: &str, value: &str, action: &str, applications: &[RuleApplication],
) -> Result<(v2rayr_store::RuleKind, String, v2rayr_store::RuleAction, Vec<RuleApplication>), Problem> {
    let applications = normalize_applications(applications)?;
    let kind = v2rayr_store::RuleKind::parse(kind).ok_or_else(|| {
        Problem::new(ErrorCode::InvalidValue.as_str(), false)
            .with_detail(format!("未知的规则类型：{kind}"))
    })?;
    let action = v2rayr_store::RuleAction::parse(action).ok_or_else(|| {
        Problem::new(ErrorCode::InvalidValue.as_str(), false)
            .with_detail(format!("未知的规则动作：{action}"))
    })?;
    let value = normalize_rule_value(value)?;
    Ok((kind, value, action, applications))
}

pub(crate) fn tun_rule(row: &v2rayr_store::RuleRow) -> Result<v2rayr_platform::tun::TunRule, Problem> {
    use v2rayr_platform::tun::{TunRule, TunRuleAction, TunRuleTarget};
    use v2rayr_store::{RuleAction, RuleKind};

    let target = match row.kind {
        RuleKind::Domain if row.value == "*" && !row.applications.is_empty() => TunRuleTarget::Any,
        RuleKind::Domain => TunRuleTarget::Domain(row.value.clone()),
        RuleKind::DomainSuffix => TunRuleTarget::DomainSuffix(row.value.clone()),
        RuleKind::IpCidr => TunRuleTarget::IpCidr(row.value.clone()),
        _ => return Err(Problem::config(format!(
            "TUN 规则分流暂不支持 {}，请停用该规则：{}", row.kind.as_str(), row.value,
        ))),
    };
    let applications = row.applications.iter().map(|application| {
        let actual = executable_identity(std::path::Path::new(&application.path));
        if !actual.is_ok_and(|actual| &actual == application) {
            return Err(Problem::config(format!("应用已移动或更新，请重新录入规则：{}", application.path)));
        }
        Ok(application.path.clone())
    }).collect::<Result<Vec<_>, _>>()?;
    let action = match row.action {
        RuleAction::Proxy => TunRuleAction::Proxy,
        RuleAction::Direct => TunRuleAction::Direct,
        RuleAction::Block => TunRuleAction::Block,
    };
    Ok(TunRule { applications, target, action })
}

pub(crate) fn pac_overrides(rules: &mut [v2rayr_config::CustomRule]) -> Vec<v2rayr_platform::PacOverride> {
    use v2rayr_config::{RuleAction, RuleKind};
    use v2rayr_platform::{PacMatch, PacOverride};

    rules.iter_mut().filter_map(|rule| {
        let target = match rule.kind {
            RuleKind::Domain | RuleKind::DomainSuffix => {
                let domain = rule.value.trim_end_matches('.').to_ascii_lowercase();
                let domain = if rule.kind == RuleKind::DomainSuffix {
                    domain.trim_start_matches('.')
                } else {
                    &domain
                };
                if domain.is_empty() || domain == "*" {
                    return None;
                }
                rule.value = domain.to_owned();
                if rule.kind == RuleKind::Domain {
                    PacMatch::Domain(rule.value.clone())
                } else {
                    PacMatch::DomainSuffix(rule.value.clone())
                }
            }
            RuleKind::IpCidr => {
                let (address, bits) = rule.value.split_once('/')?;
                let address: std::net::IpAddr = address.parse().ok()?;
                let bits = bits.parse::<u8>().ok()?;
                if bits > if address.is_ipv4() { 32 } else { 128 } {
                    return None;
                }
                PacMatch::Ip(address, bits)
            }
            _ => return None,
        };
        Some(PacOverride { target, direct: rule.action == RuleAction::Direct })
    }).collect()
}

pub(crate) fn into_config_rule(row: v2rayr_store::RuleRow) -> v2rayr_config::CustomRule {
    use v2rayr_config::{CustomRule, RuleAction};
    CustomRule {
        kind: crate::config_rule_kind(row.kind),
        value: row.value,
        action: match row.action {
            v2rayr_store::RuleAction::Proxy => RuleAction::Proxy,
            v2rayr_store::RuleAction::Direct => RuleAction::Direct,
            v2rayr_store::RuleAction::Block => RuleAction::Block,
        },
    }
}

pub(crate) fn normalize_rule_value(value: &str) -> Result<String, Problem> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Problem::new(ErrorCode::InvalidValue.as_str(), false)
            .with_detail("规则内容不能为空".to_owned()));
    }
    if value.len() > 256 || value.contains('\n') || value.contains('\r') {
        return Err(Problem::new(ErrorCode::InvalidValue.as_str(), false)
            .with_detail("规则内容过长或包含换行".to_owned()));
    }
    Ok(value.to_owned())
}

pub(crate) fn normalize_applications(items: &[RuleApplication]) -> Result<Vec<RuleApplication>, Problem> {
    if items.len() > 32 || items.iter().any(|item| {
        !(item.path.starts_with('/')
            || item.path.starts_with(r"\\")
            || item.path.as_bytes().get(..3).is_some_and(|prefix| {
                prefix[0].is_ascii_alphabetic() && prefix[1] == b':' && matches!(prefix[2], b'/' | b'\\')
            }))
            || item.path.len() > 4096
            || item.path.chars().any(char::is_control)
            || item.sha256.len() != 64
            || !item.sha256.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) {
        return Err(Problem::config("规则包含无效的应用身份"));
    }
    let mut items = items.to_vec();
    items.sort_unstable();
    items.dedup();
    Ok(items)
}

/// Resolve native picker selections without trusting display names or IPC paths
/// as identities. A bundle offers its main executable and nested app helpers.
pub fn inspect_rule_applications(path: &std::path::Path) -> Result<Vec<RuleApplication>, Problem> {
    #[cfg(target_os = "macos")]
    if path.is_dir() && path.extension().is_some_and(|ext| ext == "app") {
        let bundle = path.canonicalize().map_err(error::internal)?;
        let main = bundle_executable(&bundle)?;
        let mut applications = vec![executable_identity(&main)?];
        collect_bundle_helpers(&bundle, &bundle.join("Contents"), 8, &mut applications);
        return Ok(applications);
    }

    Ok(vec![executable_identity(path)?])
}

fn executable_identity(path: &std::path::Path) -> Result<RuleApplication, Problem> {
    use sha2::{Digest, Sha256};

    let path = path.canonicalize().map_err(error::internal)?;
    let metadata = path.metadata().map_err(error::internal)?;
    if !metadata.is_file() {
        return Err(Problem::config("请选择应用或可执行文件"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(Problem::config("请选择可执行文件"));
        }
    }
    #[cfg(windows)]
    if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe")) {
        return Err(Problem::config("请选择 .exe 文件"));
    }

    let mut file = std::fs::File::open(&path).map_err(error::internal)?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).map_err(error::internal)?;
    let identity = RuleApplication {
        path: path.to_str().ok_or_else(|| Problem::config("应用路径无法读取"))?.to_owned(),
        sha256: format!("{:x}", hash.finalize()),
    };
    normalize_applications(std::slice::from_ref(&identity))?;
    Ok(identity)
}

#[cfg(target_os = "macos")]
fn bundle_executable(bundle: &std::path::Path) -> Result<std::path::PathBuf, Problem> {
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleExecutable", "raw", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()
        .map_err(error::internal)?;
    let name = std::str::from_utf8(&output.stdout).unwrap_or_default().trim_end();
    let executable = std::path::Path::new(name);
    if !output.status.success()
        || !matches!(executable.components().next(), Some(std::path::Component::Normal(_)))
        || executable.components().count() != 1
    {
        return Err(Problem::config("应用包未声明有效的主程序"));
    }
    let path = bundle.join("Contents/MacOS").join(executable).canonicalize()
        .map_err(error::internal)?;
    if !path.starts_with(bundle) {
        return Err(Problem::config("应用包的可执行文件指向包外"));
    }
    Ok(path)
}

#[cfg(target_os = "macos")]
fn collect_bundle_helpers(
    bundle: &std::path::Path,
    directory: &std::path::Path,
    depth: usize,
    applications: &mut Vec<RuleApplication>,
) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_unstable_by_key(|entry| entry.file_name());
    for entry in entries {
        if applications.len() >= 32 {
            break;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "app" || ext == "xpc")
            && let Ok(executable) = bundle_executable(&path)
            && executable.starts_with(bundle)
            && let Ok(identity) = executable_identity(&executable)
            && !applications.contains(&identity)
        {
            applications.push(identity);
        }
        collect_bundle_helpers(bundle, &path, depth - 1, applications);
    }
}

fn rule_view(row: v2rayr_store::RuleRow) -> RuleView {
    RuleView {
        id: row.id,
        kind: row.kind.as_str().to_owned(),
        value: row.value,
        applications: row.applications,
        action: row.action.as_str().to_owned(),
        enabled: row.enabled,
        position: row.position,
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::temp_app;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_same_name_bundles_and_helpers_have_independent_identities() {
        use super::inspect_rule_applications;
        use std::{fs, path::Path};

        fn bundle(path: &Path, executable: &str) {
            fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
            fs::write(
                path.join("Contents/Info.plist"),
                format!("<plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>{executable}</string></dict></plist>"),
            ).unwrap();
            fs::copy("/usr/bin/true", path.join("Contents/MacOS").join(executable)).unwrap();
        }

        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("one/Client.app");
        let second = temp.path().join("two/Client.app");
        let helper = first.join("Contents/Frameworks/Client Helper.app");
        bundle(&first, "Client");
        bundle(&second, "Client");
        bundle(&helper, "Client Helper");

        let first_items = inspect_rule_applications(&first).unwrap();
        let second_items = inspect_rule_applications(&second).unwrap();
        let helper_items = inspect_rule_applications(&helper).unwrap();
        assert_eq!(first_items.len(), 2);
        assert_eq!(second_items.len(), 1);
        assert_eq!(first_items[0].sha256, second_items[0].sha256);
        assert_ne!(first_items[0], second_items[0]);
        assert_eq!(first_items[1], helper_items[0]);
        assert_ne!(first_items[0], helper_items[0]);

        let app = temp_app();
        let rows = app.add_rule_for_applications("domain", "example.com", "proxy", &helper_items).unwrap();
        assert_eq!(rows[0].applications, helper_items);
        let rows = app.add_rule("domain", "example.com", "direct").unwrap();
        assert!(rows[1].applications.is_empty());

        fs::copy("/usr/bin/false", first.join("Contents/MacOS/Client")).unwrap();
        let updated = inspect_rule_applications(&first).unwrap();
        assert_eq!(updated[0].path, first_items[0].path);
        assert_ne!(updated[0].sha256, first_items[0].sha256);
    }

    #[test]
    fn editing_keeps_rule_order_and_enabled_state() {
        let app = temp_app();
        let first = app.add_rule("domain", "first.example", "proxy").unwrap()[0].clone();
        app.add_rule("domain", "second.example", "direct").unwrap();
        app.set_rule_enabled(first.id, false).unwrap();
        let rules = app.update_rule(first.id, "domain-suffix", "edited.example", "block", &[]).unwrap();
        assert_eq!(rules[0].id, first.id);
        assert_eq!(rules[0].position, first.position);
        assert!(!rules[0].enabled);
        assert_eq!(rules[0].kind, "domain-suffix");
        assert_eq!(rules[0].value, "edited.example");
        assert_eq!(rules[0].action, "block");
        assert_eq!(rules[1].value, "second.example");
    }

    #[test]
    fn rules_round_trip_and_reject_bad_input() {
        let app = temp_app();
        assert!(app.rules().expect("list").is_empty());

        let rules = app
            .add_rule("domain-suffix", "example.com", "direct")
            .expect("add");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].kind, "domain-suffix");
        assert_eq!(rules[0].action, "direct");
        assert!(rules[0].enabled);

        let id = rules[0].id;
        let rules = app.set_rule_enabled(id, false).expect("disable");
        assert!(!rules[0].enabled);

        let error = app.add_rule("nope", "x", "direct").expect_err("kind");
        assert_eq!(error.code, "invalid_value");
        let error = app.add_rule("domain", "   ", "direct").expect_err("empty");
        assert_eq!(error.code, "invalid_value");
        let error = app
            .add_rule("domain", "a.com\nb.com", "direct")
            .expect_err("newline");
        assert_eq!(error.code, "invalid_value");

        let rules = app.remove_rule(id).expect("remove");
        assert!(rules.is_empty());
        assert_eq!(
            app.remove_rule(id).expect_err("gone").code,
            "not_found".to_owned()
        );
    }
}
