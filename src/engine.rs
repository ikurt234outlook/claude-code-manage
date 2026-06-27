//! 引擎层：JIT 配置生成 + `exec` 进程替换。
//!
//! 通过 `CommandExt::exec` 让 `claude` 进程在底层覆盖当前 `ccm`，
//! 避免 ccm 残留为父进程，并完美继承 stdin/stdout/stderr。

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};

use crate::config::Profile;
use crate::crypto;
use crate::paths;

/// 纯函数：为 `ccm add` 流程构造一份标准 claude settings JSON。
///
/// 字段规则（对齐 cc-switch 主流命名，认证用 `ANTHROPIC_AUTH_TOKEN` 而非 API_KEY）：
/// - `env.ANTHROPIC_AUTH_TOKEN` 始终写入
/// - `env.ANTHROPIC_BASE_URL` 仅在 `base_url` 非空时写入
/// - `env.ANTHROPIC_MODEL` 仅在 `model` 非空时写入
pub fn build_settings_json(auth_token: &str, base_url: &str, model: &str) -> Value {
    let mut env = Map::new();
    env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!(auth_token));
    if !base_url.is_empty() {
        env.insert("ANTHROPIC_BASE_URL".into(), json!(base_url));
    }
    if !model.is_empty() {
        env.insert("ANTHROPIC_MODEL".into(), json!(model));
    }
    let mut root = Map::new();
    root.insert("env".into(), Value::Object(env));
    Value::Object(root)
}

/// 把 settings JSON 文本写到 `~/.config/ccm/settings/<id>.json`，权限强制 0600。
///
/// 复用 [`crypto::write_secure`]（design §5 的统一安全写入路径），与 `.master_key`
/// 共用同一套「写入 + 强制 0600」逻辑，避免权限处理重复实现。
fn write_jit_settings(profile_id: &str, settings_text: &str) -> Result<PathBuf> {
    let path = paths::settings_file(profile_id)?;
    crypto::write_secure(&path, settings_text.as_bytes())
        .with_context(|| format!("写入 settings 文件失败: {}", path.display()))?;
    Ok(path)
}

/// 启动入口：
/// 1. 解密 `encrypted_settings` 得到完整 settings JSON 明文
/// 2. 原样写出 JIT settings 文件并 0600 落盘
/// 3. `claude --settings <path>` 经 `exec` 接管进程
///
/// `exec` 成功后**不会返回**；返回即代表错误（PATH 中无 claude 等）。
pub fn launch(profile: &Profile, master_key: &[u8; 32]) -> Result<()> {
    let settings_text = crypto::decrypt(&profile.encrypted_settings, master_key)
        .context("解密 settings 失败 (master_key 可能已变更)")?;
    // 校验明文确为合法 JSON，避免把损坏内容喂给 claude。
    serde_json::from_str::<Value>(&settings_text).context("解密得到的 settings 不是合法 JSON")?;
    let path = write_jit_settings(&profile.id, &settings_text)?;
    exec_claude(&path)
}

fn exec_claude(settings_path: &Path) -> Result<()> {
    let mut cmd = Command::new("claude");
    cmd.arg("--settings").arg(settings_path);
    // exec() 在成功时不会返回；返回的 io::Error 一定代表执行失败。
    let err = cmd.exec();
    Err(anyhow!(
        "无法执行 `claude`：{err}。请确认 claude 已安装并在 PATH 中。"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_fields() {
        let v = build_settings_json("sk-tok", "https://api.example.com", "claude-opus-4-8");
        assert_eq!(v["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-tok");
        assert_eq!(v["env"]["ANTHROPIC_BASE_URL"], "https://api.example.com");
        assert_eq!(v["env"]["ANTHROPIC_MODEL"], "claude-opus-4-8");
        // 不应写出 API_KEY（cc-switch 主流用 AUTH_TOKEN）
        assert!(v["env"].get("ANTHROPIC_API_KEY").is_none());
    }

    #[test]
    fn skips_empty_optional_fields() {
        let v = build_settings_json("sk-tok", "", "");
        assert_eq!(v["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-tok");
        assert!(v["env"].get("ANTHROPIC_BASE_URL").is_none());
        assert!(v["env"].get("ANTHROPIC_MODEL").is_none());
    }

    #[test]
    fn base_url_only() {
        let v = build_settings_json("k", "https://x", "");
        assert_eq!(v["env"]["ANTHROPIC_BASE_URL"], "https://x");
        assert!(v["env"].get("ANTHROPIC_MODEL").is_none());
    }
}
