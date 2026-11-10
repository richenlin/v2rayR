//! Custom routing rules.
//!
//! Rules are stored in SQLite and read when a session plan is built, so an edit
//! takes effect on the next connect. The UI says so instead of pretending the
//! running session changed.

use v2rayr_domain::ErrorCode;
use v2rayr_ipc::Problem;
use v2rayr_ipc::RuleView;

use crate::App;
use crate::error;

impl App {
    pub fn rules(&self) -> Result<Vec<RuleView>, Problem> {
        let rows = self.db().list_rules().map_err(error::from_store)?;
        Ok(rows.into_iter().map(rule_view).collect())
    }

    pub fn add_rule(
        &self,
        kind: &str,
        value: &str,
        action: &str,
    ) -> Result<Vec<RuleView>, Problem> {
        let kind = v2rayr_store::RuleKind::parse(kind).ok_or_else(|| {
            Problem::new(ErrorCode::InvalidValue.as_str(), false)
                .with_detail(format!("未知的规则类型：{kind}"))
        })?;
        let action = v2rayr_store::RuleAction::parse(action).ok_or_else(|| {
            Problem::new(ErrorCode::InvalidValue.as_str(), false)
                .with_detail(format!("未知的规则动作：{action}"))
        })?;
        let value = normalize_rule_value(value)?;
        self.db()
            .add_rule(kind, &value, action)
            .map_err(error::from_store)?;
        self.logs
            .info("rules", format!("新增规则 {} {}", kind.as_str(), value));
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

fn rule_view(row: v2rayr_store::RuleRow) -> RuleView {
    RuleView {
        id: row.id,
        kind: row.kind.as_str().to_owned(),
        value: row.value,
        action: row.action.as_str().to_owned(),
        enabled: row.enabled,
        position: row.position,
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::temp_app;

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
