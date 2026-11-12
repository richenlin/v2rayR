//! Kernel configuration emission.
//!
//! The app never writes JSON/YAML by hand: it builds a [`SessionPlan`], asks a
//! kernel-specific emitter for a config, then validates the result (unique tags,
//! every route target exists, sane ports) before anything reaches disk. That
//! ordering is the whole point of this crate — a bad config must fail here, not
//! as a kernel crash after the UI already said "已连接".

pub mod application_ingress;
pub mod capability;
pub mod mihomo;
pub mod plan;
pub mod routing;
pub mod singbox;
pub mod validate;
pub mod xray;
mod yaml;

use thiserror::Error;
use v2rayr_domain::{CoreKind, ErrorCode};

pub use capability::{Capability, capabilities, choose_core};
pub use plan::{
    ApiPlan, CustomRule, DnsPlan, InboundPlan, LAN_USERNAME, LanInboundPlan, RoutingMode,
    RuleAction, RuleKind, RuleSetKind, RuleSetRef, SessionPlan, TunPlan,
};
pub use validate::validate;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("no kernel can run this node: {0}")]
    NoCapableKernel(String),
    #[error("{core} cannot run this node: {reason}")]
    Unsupported { core: &'static str, reason: String },
    #[error("invalid plan: {0}")]
    InvalidPlan(String),
    #[error("emitted config is inconsistent: {0}")]
    Inconsistent(String),
    #[error("serialisation failed: {0}")]
    Serialize(#[from] serde_json::Error),
}

impl ConfigError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::NoCapableKernel(_) | Self::Unsupported { .. } => ErrorCode::UnsupportedProtocol,
            Self::InvalidPlan(_) => ErrorCode::InvalidValue,
            Self::Inconsistent(_) => ErrorCode::ConfigInvalid,
            Self::Serialize(_) => ErrorCode::Internal,
        }
    }
}

/// Wire format of an emitted config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Yaml,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }
}

/// A rendered config ready to be written to the kernel's config path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmittedConfig {
    pub core: CoreKind,
    pub format: Format,
    pub body: String,
    /// Things the user should know and the UI may surface ("Xray runs SOCKS on
    /// 10808 and HTTP on 10809").
    pub notes: Vec<String>,
}

impl EmittedConfig {
    pub fn new(core: CoreKind, format: Format, body: String) -> Self {
        Self {
            core,
            format,
            body,
            notes: Vec::new(),
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Validate the body and return it, or the structural problem that makes it
    /// unusable.
    pub fn validated(self) -> Result<Self, ConfigError> {
        validate(self.core, self.format, &self.body)?;
        Ok(self)
    }
}

/// Emit a config for whichever kernel the capability matrix picks.
pub fn emit(plan: &SessionPlan) -> Result<EmittedConfig, ConfigError> {
    plan.check()?;
    let core = choose_core(&plan.profile, plan.core_override)?;
    emit_for(core, plan)
}

pub fn emit_for(core: CoreKind, plan: &SessionPlan) -> Result<EmittedConfig, ConfigError> {
    let emitted = match core {
        CoreKind::SingBox => singbox::emit(plan)?,
        CoreKind::Xray => xray::emit(plan)?,
        CoreKind::Mihomo => mihomo::emit(plan)?,
    };
    emitted.validated()
}
