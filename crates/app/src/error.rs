//! Mapping typed errors onto the one shape the UI understands.

use v2rayr_config::ConfigError;
use v2rayr_domain::{DomainError, ErrorCode};
use v2rayr_engine::EngineError;
use v2rayr_ipc::Problem;
use v2rayr_platform::PlatformError;
use v2rayr_store::StoreError;

pub fn from_domain(error: DomainError) -> Problem {
    Problem::from(error)
}

pub fn from_store(error: StoreError) -> Problem {
    match error {
        StoreError::Domain(error) => Problem::from(error),
        StoreError::NotFound(id) => {
            Problem::new(ErrorCode::NotFound.as_str(), false).with_detail(id)
        }
        StoreError::SchemaTooNew { found, supported } => Problem::storage(format!(
            "数据库版本 {found} 高于本版本支持的 {supported}，请升级 v2rayR"
        )),
        other => Problem::storage(other.to_string()),
    }
}

pub fn from_config(error: ConfigError) -> Problem {
    match error {
        ConfigError::NoCapableKernel(reason) => {
            Problem::new(ErrorCode::CoreMissing.as_str(), false).with_detail(if reason.is_empty() {
                "没有已安装的内核支持该节点".to_owned()
            } else {
                reason
            })
        }
        ConfigError::Unsupported { core, reason } => {
            Problem::new(ErrorCode::UnsupportedProtocol.as_str(), false)
                .with_detail(format!("{core}: {reason}"))
        }
        ConfigError::InvalidPlan(reason) => {
            Problem::new(ErrorCode::InvalidValue.as_str(), false).with_detail(reason)
        }
        ConfigError::Inconsistent(message) => Problem::config(message),
        ConfigError::Serialize(error) => Problem::internal(error.to_string()),
    }
}

pub fn from_engine(error: EngineError) -> Problem {
    let retryable = matches!(
        error.code(),
        ErrorCode::NetworkError | ErrorCode::CoreFailed | ErrorCode::Storage
    );
    let code = error.code();
    let detail = if code == ErrorCode::CoreMissing {
        "未找到可用的代理内核；请安装 sing-box 或 Xray，并在设置中指定内核路径".to_owned()
    } else {
        error.to_string()
    };
    Problem::new(code.as_str(), retryable).with_detail(detail)
}

pub fn from_platform(error: PlatformError) -> Problem {
    Problem::new(error.code().as_str(), false).with_detail(error.to_string())
}

pub fn internal(context: impl std::fmt::Display) -> Problem {
    Problem::internal(context.to_string())
}
