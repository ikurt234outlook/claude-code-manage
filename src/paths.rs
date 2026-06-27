//! 核心路径解析与目录初始化。
//!
//! 所有 ccm 的持久化状态都集中在 `~/.config/ccm/` 下：
//!   - `.master_key`        AES-256 根密钥（0600）
//!   - `profiles.json`      加密后的环境配置
//!   - `settings/<id>.json` JIT 运行时生成的明文配置（0600）

use anyhow::{Context, Result};
use std::path::PathBuf;

/// XDG 配置根目录：`$XDG_CONFIG_HOME`（若非空）否则 `$HOME/.config`。
///
/// 注意：不使用 `dirs::config_dir()`——它在 macOS 上返回
/// `~/Library/Application Support`，与本项目规格（`~/.config/ccm/`）不一致。
/// （cc-switch 兼容数据源是另一路径 `~/.cc-switch/`，见 [`cc_switch_db`]。）
fn xdg_config_home() -> Result<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Ok(PathBuf::from(x));
        }
    }
    let home = dirs::home_dir().context("无法解析用户主目录（$HOME）")?;
    Ok(home.join(".config"))
}

/// ccm 核心配置目录：`~/.config/ccm/`。
pub fn config_dir() -> Result<PathBuf> {
    Ok(xdg_config_home()?.join("ccm"))
}

/// 根密钥路径：`~/.config/ccm/.master_key`。
pub fn master_key_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(".master_key"))
}

/// 主数据源路径：`~/.config/ccm/profiles.json`。
pub fn profiles_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("profiles.json"))
}

/// JIT 运行时目录：`~/.config/ccm/settings/`。
pub fn settings_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("settings"))
}

/// 单个 profile 的 JIT 配置文件路径：`~/.config/ccm/settings/<id>.json`。
pub fn settings_file(id: &str) -> Result<PathBuf> {
    Ok(settings_dir()?.join(format!("{id}.json")))
}

/// cc-switch 兼容数据源：`~/.cc-switch/cc-switch.db`。
///
/// 经实测，cc-switch 的数据库位于 `$HOME/.cc-switch/cc-switch.db`（直接在 HOME
/// 根下，**不**在 `.config` 内），文件名为 `cc-switch.db`，而非 prompt 假设的
/// `~/.config/cc-switch/database.sqlite`。
pub fn cc_switch_db() -> Result<PathBuf> {
    let home = dirs::home_dir().context("无法解析用户主目录（$HOME）")?;
    Ok(home.join(".cc-switch").join("cc-switch.db"))
}

/// 确保核心目录与运行时目录存在，不存在则递归创建。
pub fn ensure_dirs() -> Result<()> {
    let cfg = config_dir()?;
    std::fs::create_dir_all(&cfg).with_context(|| format!("创建配置目录失败: {}", cfg.display()))?;
    let settings = settings_dir()?;
    std::fs::create_dir_all(&settings)
        .with_context(|| format!("创建运行时目录失败: {}", settings.display()))?;
    Ok(())
}
